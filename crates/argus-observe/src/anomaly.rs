//! Process-level anomaly detection over inventory snapshots and lifecycle
//! changes (CAP-2). These are mechanical anomalies observable purely from
//! inventory dynamics; "unexpected binary/listener" — which need an allowlist
//! baseline — belong to the risk engine (`argus-risk`).

use crate::inventory::{LifecycleChange, ProcessInventory};

/// Detection thresholds. All are operator-configurable; [`Default`] values are
/// conservative placeholders, not tuned production settings.
#[derive(Debug, Clone, Copy)]
pub struct AnomalyConfig {
    /// Processes started in one interval above which we flag a fork explosion.
    pub explosion_threshold: usize,
    /// (started + exited) in one interval above which we flag churn.
    pub churn_threshold: usize,
    /// CPU-ticks delta in one interval above which we flag a runaway process.
    pub cpu_ticks_threshold: u64,
}

impl Default for AnomalyConfig {
    fn default() -> Self {
        Self {
            explosion_threshold: 50,
            churn_threshold: 100,
            cpu_ticks_threshold: 10_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessAnomalyKind {
    /// Many processes started in one interval (fork bomb / rapid spawn).
    Explosion,
    /// High process start+exit turnover.
    Churn,
    /// A single process consumed an abnormal amount of CPU in one interval.
    Runaway,
}

/// An evidence-backed process anomaly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessAnomaly {
    pub kind: ProcessAnomalyKind,
    /// The offending process, where the anomaly is attributable to one.
    pub pid: Option<u32>,
    /// Deterministic detail, never generated text.
    pub detail: String,
}

/// Detect lifecycle anomalies (explosion, churn) from a tick's changes.
pub fn detect_lifecycle_anomalies(
    config: &AnomalyConfig,
    changes: &[LifecycleChange],
) -> Vec<ProcessAnomaly> {
    let started = changes
        .iter()
        .filter(|c| matches!(c, LifecycleChange::Started { .. }))
        .count();
    let exited = changes
        .iter()
        .filter(|c| matches!(c, LifecycleChange::Exited { .. }))
        .count();

    let mut out = Vec::new();
    if started >= config.explosion_threshold {
        out.push(ProcessAnomaly {
            kind: ProcessAnomalyKind::Explosion,
            pid: None,
            detail: format!("{started} processes started in one interval"),
        });
    }
    if started + exited >= config.churn_threshold {
        out.push(ProcessAnomaly {
            kind: ProcessAnomalyKind::Churn,
            pid: None,
            detail: format!("{started} started, {exited} exited in one interval"),
        });
    }
    out
}

/// Detect runaway processes: per-PID CPU-tick delta above the threshold.
///
/// A PID whose `start_ticks` changed between snapshots is a *different* process
/// (PID reuse) and is skipped, since its tick counters reset.
pub fn detect_runaway(
    prev: &ProcessInventory,
    curr: &ProcessInventory,
    cpu_ticks_threshold: u64,
) -> Vec<ProcessAnomaly> {
    let mut out = Vec::new();
    for (pid, rec) in &curr.processes {
        let Some(prev_rec) = prev.processes.get(pid) else {
            continue;
        };
        if prev_rec.start_ticks != rec.start_ticks {
            continue; // PID reused; different process
        }
        let prev_cpu = prev_rec.utime + prev_rec.stime;
        let curr_cpu = rec.utime + rec.stime;
        let delta = curr_cpu.saturating_sub(prev_cpu);
        if delta > cpu_ticks_threshold {
            out.push(ProcessAnomaly {
                kind: ProcessAnomalyKind::Runaway,
                pid: Some(*pid),
                detail: format!(
                    "{} consumed {delta} CPU ticks in one interval (threshold {cpu_ticks_threshold})",
                    rec.name
                ),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(pid: u32) -> LifecycleChange {
        LifecycleChange::Started {
            pid,
            name: format!("p{pid}"),
            ppid: 1,
        }
    }

    fn exited(pid: u32) -> LifecycleChange {
        LifecycleChange::Exited {
            pid,
            name: format!("p{pid}"),
        }
    }

    #[test]
    fn explosion_flags_when_started_exceeds_threshold() {
        let cfg = AnomalyConfig {
            explosion_threshold: 3,
            churn_threshold: 100,
            cpu_ticks_threshold: 0,
        };
        let changes = vec![started(1), started(2), started(3)];
        let a = detect_lifecycle_anomalies(&cfg, &changes);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, ProcessAnomalyKind::Explosion);
    }

    #[test]
    fn churn_flags_on_high_turnover() {
        let cfg = AnomalyConfig {
            explosion_threshold: 50,
            churn_threshold: 4,
            cpu_ticks_threshold: 0,
        };
        let changes = vec![started(1), started(2), exited(3), exited(4)];
        let a = detect_lifecycle_anomalies(&cfg, &changes);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, ProcessAnomalyKind::Churn);
    }

    #[test]
    fn below_threshold_is_quiet() {
        let cfg = AnomalyConfig::default();
        let changes = vec![started(1), exited(2)];
        assert!(detect_lifecycle_anomalies(&cfg, &changes).is_empty());
    }

    fn rec(pid: u32, start: u64, cpu: u64) -> crate::ProcessRecord {
        crate::ProcessRecord {
            pid,
            ppid: 1,
            name: format!("p{pid}"),
            state: "R".to_string(),
            uid: 0,
            threads: 1,
            vm_rss_kib: 0,
            cap_eff: 0,
            utime: cpu,
            stime: 0,
            start_ticks: start,
            cmdline: Vec::new(),
            cgroup: Vec::new(),
        }
    }

    #[test]
    fn runaway_detects_cpu_delta_above_threshold() {
        let mut prev = ProcessInventory::new();
        prev.insert(rec(1, 100, 1000));
        let mut curr = ProcessInventory::new();
        curr.insert(rec(1, 100, 2500)); // +1500 ticks

        let a = detect_runaway(&prev, &curr, 1000);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].kind, ProcessAnomalyKind::Runaway);
        assert_eq!(a[0].pid, Some(1));
    }

    #[test]
    fn runaway_skips_reused_pids() {
        let mut prev = ProcessInventory::new();
        prev.insert(rec(1, 100, 1000));
        // Same pid, different start_ticks: a reused PID, not the same process.
        let mut curr = ProcessInventory::new();
        curr.insert(rec(1, 999, 0));

        assert!(detect_runaway(&prev, &curr, 0).is_empty());
    }
}
