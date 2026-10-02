//! The synchronous observation coordinator (ADR-0032 §2).
//!
//! One [`Observer::tick`] snapshots the process inventory, diffs it against the
//! previous tick, and detects lifecycle + runaway anomalies. The async schedule
//! (periodic tick + event publishing) is the daemon's job; this coordinator is
//! synchronous and deterministic so it is testable without a host or a model.

use crate::ObserveError;
use crate::anomaly::{AnomalyConfig, ProcessAnomaly, detect_lifecycle_anomalies, detect_runaway};
use crate::inventory::{LifecycleChange, ProcessInventory, diff};
use crate::snapshot::ProcSnapshotter;

/// The result of one observation tick.
#[derive(Debug, Clone)]
pub struct TickOutput {
    pub inventory: ProcessInventory,
    pub changes: Vec<LifecycleChange>,
    pub anomalies: Vec<ProcessAnomaly>,
}

/// The observation coordinator: owns the running inventory and advances it one
/// tick at a time.
#[derive(Debug)]
pub struct Observer {
    snapshotter: ProcSnapshotter,
    inventory: ProcessInventory,
    anomaly_config: AnomalyConfig,
    /// True after the first tick, which only establishes the baseline inventory.
    warmed: bool,
}

impl Observer {
    pub fn new(snapshotter: ProcSnapshotter, anomaly_config: AnomalyConfig) -> Self {
        Self {
            snapshotter,
            inventory: ProcessInventory::new(),
            anomaly_config,
            warmed: false,
        }
    }

    /// One synchronous tick: snapshot → diff → detect anomalies.
    pub fn tick(&mut self) -> Result<TickOutput, ObserveError> {
        let current = self.snapshotter.inventory()?;

        // The first tick only establishes the baseline inventory. Reporting every
        // pre-existing process as "started" would trigger a false process-explosion
        // anomaly on cold start.
        let changes = if self.warmed {
            diff(&self.inventory, &current)
        } else {
            Vec::new()
        };

        let mut anomalies = detect_lifecycle_anomalies(&self.anomaly_config, &changes);
        anomalies.extend(detect_runaway(
            &self.inventory,
            &current,
            self.anomaly_config.cpu_ticks_threshold,
        ));

        let output = TickOutput {
            inventory: current.clone(),
            changes,
            anomalies,
        };
        self.inventory = current;
        self.warmed = true;
        Ok(output)
    }

    pub fn inventory(&self) -> &ProcessInventory {
        &self.inventory
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::LifecycleChange;
    use std::path::Path;

    const STATUS: &str = "Name:\tproc\nState:\tS (sleeping)\nPid:\t__PID__\nPPid:\t1\nUid:\t0\t0\t0\t0\nThreads:\t1\nVmRSS:\t0 kB\nCapEff:\t0000000000000000\n";

    fn unique_root() -> std::path::PathBuf {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "argus-observe-observer-test-{}-{n}",
            std::process::id()
        ))
    }

    fn write_pid(root: &Path, pid: u32) {
        let dir = root.join(pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("status"),
            STATUS.replace("__PID__", &pid.to_string()),
        )
        .unwrap();
    }

    #[test]
    fn first_tick_has_no_changes() {
        let root = unique_root();
        write_pid(&root, 1234);
        let mut obs = Observer::new(ProcSnapshotter::with_root(&root), AnomalyConfig::default());
        let out = obs.tick().unwrap();
        assert_eq!(out.inventory.len(), 1);
        assert!(out.changes.is_empty());
        assert!(out.anomalies.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_processes_started_between_ticks() {
        let root = unique_root();
        write_pid(&root, 1234);
        let mut obs = Observer::new(ProcSnapshotter::with_root(&root), AnomalyConfig::default());
        obs.tick().unwrap();

        write_pid(&root, 5678);
        let out = obs.tick().unwrap();
        assert_eq!(out.inventory.len(), 2);
        assert_eq!(
            out.changes,
            vec![LifecycleChange::Started {
                pid: 5678,
                name: "proc".to_string(),
                ppid: 1,
            }]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_processes_exited_between_ticks() {
        let root = unique_root();
        write_pid(&root, 1234);
        write_pid(&root, 5678);
        let mut obs = Observer::new(ProcSnapshotter::with_root(&root), AnomalyConfig::default());
        obs.tick().unwrap();

        std::fs::remove_dir_all(root.join("5678")).unwrap();
        let out = obs.tick().unwrap();
        assert_eq!(out.inventory.len(), 1);
        assert_eq!(
            out.changes,
            vec![LifecycleChange::Exited {
                pid: 5678,
                name: "proc".to_string(),
            }]
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
