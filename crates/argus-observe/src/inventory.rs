//! The live process inventory and its lifecycle diff.

use std::collections::BTreeMap;

/// A single process as observed in one tick (CAP-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRecord {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    /// Process state letter (`S`, `R`, `Z`, …).
    pub state: String,
    /// Real UID.
    pub uid: u32,
    pub threads: u32,
    pub vm_rss_kib: u64,
    /// Effective capabilities bitmap.
    pub cap_eff: u64,
    /// User-mode CPU ticks (cumulative).
    pub utime: u64,
    /// Kernel-mode CPU ticks (cumulative).
    pub stime: u64,
    /// Start time in ticks since boot; distinguishes a reused PID.
    pub start_ticks: u64,
    pub cmdline: Vec<String>,
    pub cgroup: Vec<String>,
}

/// A snapshot of every process, keyed by PID.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessInventory {
    pub processes: BTreeMap<u32, ProcessRecord>,
}

impl ProcessInventory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, rec: ProcessRecord) {
        self.processes.insert(rec.pid, rec);
    }

    pub fn get(&self, pid: u32) -> Option<&ProcessRecord> {
        self.processes.get(&pid)
    }

    pub fn len(&self) -> usize {
        self.processes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.processes.is_empty()
    }
}

/// A field whose change is a meaningful lifecycle signal.
///
/// CPU counters, thread count, and RSS are metrics and are intentionally *not*
/// tracked here — they change every tick and would drown out real lifecycle
/// events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Ppid,
    State,
    Cmdline,
    Cgroup,
}

/// A lifecycle change between two ticks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleChange {
    Started { pid: u32, name: String, ppid: u32 },
    Exited { pid: u32, name: String },
    Changed { pid: u32, name: String, fields: Vec<Field> },
}

/// Diff two snapshots into lifecycle changes.
pub fn diff(prev: &ProcessInventory, curr: &ProcessInventory) -> Vec<LifecycleChange> {
    let mut changes = Vec::new();

    for (pid, rec) in &curr.processes {
        match prev.processes.get(pid) {
            None => changes.push(LifecycleChange::Started {
                pid: *pid,
                name: rec.name.clone(),
                ppid: rec.ppid,
            }),
            Some(prev_rec) => {
                let fields = changed_fields(prev_rec, rec);
                if !fields.is_empty() {
                    changes.push(LifecycleChange::Changed {
                        pid: *pid,
                        name: rec.name.clone(),
                        fields,
                    });
                }
            }
        }
    }

    for (pid, rec) in &prev.processes {
        if !curr.processes.contains_key(pid) {
            changes.push(LifecycleChange::Exited {
                pid: *pid,
                name: rec.name.clone(),
            });
        }
    }

    changes
}

fn changed_fields(prev: &ProcessRecord, curr: &ProcessRecord) -> Vec<Field> {
    let mut fields = Vec::new();
    if prev.ppid != curr.ppid {
        fields.push(Field::Ppid);
    }
    if prev.state != curr.state {
        fields.push(Field::State);
    }
    if prev.cmdline != curr.cmdline {
        fields.push(Field::Cmdline);
    }
    if prev.cgroup != curr.cgroup {
        fields.push(Field::Cgroup);
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(pid: u32, ppid: u32, state: &str, cmdline: &[&str]) -> ProcessRecord {
        ProcessRecord {
            pid,
            ppid,
            name: format!("proc{pid}"),
            state: state.to_string(),
            uid: 0,
            threads: 1,
            vm_rss_kib: 0,
            cap_eff: 0,
            utime: 0,
            stime: 0,
            start_ticks: pid as u64,
            cmdline: cmdline.iter().map(|s| s.to_string()).collect(),
            cgroup: Vec::new(),
        }
    }

    fn inventory(recs: &[ProcessRecord]) -> ProcessInventory {
        let mut inv = ProcessInventory::new();
        for r in recs {
            inv.insert(r.clone());
        }
        inv
    }

    #[test]
    fn detects_started_and_exited() {
        let prev = inventory(&[rec(1, 0, "S", &["init"])]);
        let curr = inventory(&[rec(1, 0, "S", &["init"]), rec(2, 1, "R", &["worker"])]);

        let changes = diff(&prev, &curr);
        assert_eq!(
            changes,
            vec![LifecycleChange::Started {
                pid: 2,
                name: "proc2".to_string(),
                ppid: 1,
            }]
        );

        // Reverse direction: process 2 exited.
        let changes = diff(&curr, &prev);
        assert_eq!(
            changes,
            vec![LifecycleChange::Exited {
                pid: 2,
                name: "proc2".to_string(),
            }]
        );
    }

    #[test]
    fn detects_meaningful_field_changes() {
        let prev = inventory(&[rec(1, 0, "S", &["init"])]);
        // State changed and cmdline changed (exec), ppid changed (reparented).
        let mut r = rec(1, 42, "R", &["newbinary"]);
        r.threads = 99; // metric change: must NOT appear in fields
        r.vm_rss_kib = 999_999; // metric change: must NOT appear in fields
        let curr = inventory(&[r]);

        let changes = diff(&prev, &curr);
        assert_eq!(
            changes,
            vec![LifecycleChange::Changed {
                pid: 1,
                name: "proc1".to_string(),
                fields: vec![Field::Ppid, Field::State, Field::Cmdline],
            }]
        );
    }

    #[test]
    fn metric_changes_alone_are_not_lifecycle_changes() {
        let prev = inventory(&[rec(1, 0, "S", &["init"])]);
        let mut r = rec(1, 0, "S", &["init"]);
        r.utime = 1000; // CPU grew; nothing else changed
        r.threads = 7;
        let curr = inventory(&[r]);

        assert!(diff(&prev, &curr).is_empty());
    }

    #[test]
    fn no_change_yields_empty_diff() {
        let inv = inventory(&[rec(1, 0, "S", &["init"])]);
        assert!(diff(&inv, &inv).is_empty());
    }
}
