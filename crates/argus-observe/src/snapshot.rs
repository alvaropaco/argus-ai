//! The `/proc` snapshotter: assembles a [`ProcessInventory`] by reading each
//! pid's status/stat/cmdline/cgroup through `argus-sensors`.
//!
//! The root is parameterized (`with_root`) so tests can point at a fixture
//! `/proc` tree; production uses `/proc`.

use std::path::{Path, PathBuf};

use crate::ObserveError;
use crate::inventory::{ProcessInventory, ProcessRecord};

/// Reads a live process inventory from a `/proc`-shaped directory tree.
#[derive(Debug, Clone)]
pub struct ProcSnapshotter {
    root: PathBuf,
}

impl ProcSnapshotter {
    pub fn new() -> Self {
        Self::with_root("/proc")
    }

    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Enumerate numeric `/proc` entries (PIDs), sorted.
    pub fn list_pids(&self) -> Result<Vec<u32>, ObserveError> {
        let entries = std::fs::read_dir(&self.root).map_err(ObserveError::ListPids)?;
        let mut pids: Vec<u32> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()))
            .collect();
        pids.sort_unstable();
        Ok(pids)
    }

    /// Read one process. Returns `None` if it vanished mid-read (PID exited).
    pub fn snapshot(&self, pid: u32) -> Option<ProcessRecord> {
        let status = self.read(pid, "status")?;
        let st = argus_sensors::process::parse(&status).ok()?;

        // stat, cmdline, and cgroup are optional: a missing file degrades to
        // defaults rather than dropping the process from the inventory.
        let stat = self.read(pid, "stat").unwrap_or_default();
        let pst = argus_sensors::process::parse_stat(&stat).unwrap_or_default();

        let cmdline = self.read(pid, "cmdline").unwrap_or_default();
        let cmd = argus_sensors::process::parse_cmdline(&cmdline);

        let cgroup = self.read(pid, "cgroup").unwrap_or_default();
        let cg = argus_sensors::process::parse_cgroup(&cgroup);

        Some(ProcessRecord {
            pid: st.pid,
            ppid: st.ppid,
            name: st.name,
            state: st.state,
            uid: st.uid,
            threads: st.threads,
            vm_rss_kib: st.vm_rss_kib,
            cap_eff: st.cap_eff,
            utime: pst.utime,
            stime: pst.stime,
            start_ticks: pst.start_ticks,
            cmdline: cmd,
            cgroup: cg,
        })
    }

    /// Snapshot every process into an inventory.
    pub fn inventory(&self) -> Result<ProcessInventory, ObserveError> {
        let mut inv = ProcessInventory::new();
        for pid in self.list_pids()? {
            if let Some(rec) = self.snapshot(pid) {
                inv.insert(rec);
            }
        }
        Ok(inv)
    }

    fn read(&self, pid: u32, file: &str) -> Option<String> {
        let path = self.root.join(pid.to_string()).join(file);
        std::fs::read_to_string(path).ok()
    }
}

impl Default for ProcSnapshotter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = "Name:\tnginx\nState:\tS (sleeping)\nPid:\t1234\nPPid:\t1\nUid:\t0\t0\t0\t0\nThreads:\t8\nVmRSS:\t   12345 kB\nCapEff:\t0000003fffffffff\n";
    const STAT: &str = "1234 (nginx) S 1 1234 1234 0 -1 4194560 100 0 0 0 250 150 0 0 20 0 8 0 99999999 123456789 5678 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";
    const CMDLINE: &[u8] = b"nginx\0-master\0process\0";
    const CGROUP: &str = "0::/user.slice/session.scope\n";

    fn fixture_root() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("argus-observe-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(root.join("1234")).unwrap();
        std::fs::create_dir_all(root.join("5678")).unwrap();
        std::fs::create_dir_all(root.join("not-a-pid")).unwrap();

        std::fs::write(root.join("1234/status"), STATUS).unwrap();
        std::fs::write(root.join("1234/stat"), STAT).unwrap();
        std::fs::write(root.join("1234/cmdline"), CMDLINE).unwrap();
        std::fs::write(root.join("1234/cgroup"), CGROUP).unwrap();
        // 5678 has only a status file (partial): snapshot must still succeed on
        // the fields it can read, using defaults for the missing files.
        std::fs::write(root.join("5678/status"), STATUS.replace("1234", "5678")).unwrap();
        root
    }

    #[test]
    fn lists_only_numeric_pids() {
        let root = fixture_root();
        let snap = ProcSnapshotter::with_root(&root);
        assert_eq!(snap.list_pids().unwrap(), vec![1234, 5678]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshots_a_full_process() {
        let root = fixture_root();
        let snap = ProcSnapshotter::with_root(&root);
        let rec = snap.snapshot(1234).unwrap();
        assert_eq!(rec.pid, 1234);
        assert_eq!(rec.ppid, 1);
        assert_eq!(rec.name, "nginx");
        assert_eq!(rec.state, "S");
        assert_eq!(rec.threads, 8);
        assert_eq!(rec.vm_rss_kib, 12_345);
        assert_eq!(rec.cap_eff, 0x3f_ffff_ffff);
        assert_eq!(rec.utime, 250);
        assert_eq!(rec.stime, 150);
        assert_eq!(rec.start_ticks, 99_999_999);
        assert_eq!(rec.cmdline, vec!["nginx", "-master", "process"]);
        assert_eq!(rec.cgroup, vec!["0::/user.slice/session.scope"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_tolerates_missing_files() {
        let root = fixture_root();
        let snap = ProcSnapshotter::with_root(&root);
        let rec = snap.snapshot(5678).unwrap();
        assert_eq!(rec.pid, 5678);
        assert!(rec.cmdline.is_empty());
        assert!(rec.cgroup.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_returns_none_for_gone_pid() {
        let root = fixture_root();
        let snap = ProcSnapshotter::with_root(&root);
        assert!(snap.snapshot(9999).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn builds_a_full_inventory() {
        let root = fixture_root();
        let snap = ProcSnapshotter::with_root(&root);
        let inv = snap.inventory().unwrap();
        assert_eq!(inv.len(), 2);
        assert!(inv.get(1234).is_some());
        assert!(inv.get(5678).is_some());
        std::fs::remove_dir_all(root).unwrap();
    }
}
