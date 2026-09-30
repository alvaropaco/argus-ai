//! cgroup v2 reader (unified hierarchy, mounted at `/sys/fs/cgroup`).
//!
//! Reads the root cgroup's memory and CPU accounting plus the OOM-kill counter.
//! This is the kernel-native "cgroups + OOM events" source (CAP-1) and the seed
//! for container association (a process's `/proc/<pid>/cgroup` membership points
//! into this hierarchy). The root is parameterized so tests use a fixture tree.

use std::path::{Path, PathBuf};

/// Root-cgroup accounting snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CgroupSnapshot {
    pub memory_current_bytes: u64,
    /// `None` when `memory.max` is `"max"` (no limit).
    pub memory_max_bytes: Option<u64>,
    pub oom_kill: u64,
    pub cpu_usage_usec: u64,
}

/// Parse `memory.max`: `"max"` → `None`, otherwise the byte value.
pub fn parse_memory_max(input: &str) -> Option<u64> {
    let s = input.trim();
    if s == "max" {
        None
    } else {
        s.parse().ok()
    }
}

/// Extract the `oom_kill` counter from `memory.events`.
pub fn parse_memory_events(input: &str) -> u64 {
    find_counter(input, "oom_kill")
}

/// Extract the `usage_usec` counter from `cpu.stat`.
pub fn parse_cpu_stat(input: &str) -> u64 {
    find_counter(input, "usage_usec")
}

fn find_counter(input: &str, key: &str) -> u64 {
    input
        .lines()
        .find_map(|line| {
            let mut it = line.split_whitespace();
            if it.next() == Some(key) {
                it.next()?.parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(0)
}

/// Reads the root cgroup's accounting files under a `/sys/fs/cgroup`-shaped root.
#[derive(Debug, Clone)]
pub struct CgroupV2Reader {
    root: PathBuf,
}

impl CgroupV2Reader {
    pub fn new() -> Self {
        Self::with_root("/sys/fs/cgroup")
    }

    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Read the root cgroup snapshot. Missing files degrade to defaults (0 /
    /// `None`) rather than erroring (ADR-0032 §4).
    pub fn snapshot(&self) -> CgroupSnapshot {
        CgroupSnapshot {
            memory_current_bytes: read_u64(&self.root.join("memory.current")).unwrap_or(0),
            memory_max_bytes: read_str(&self.root.join("memory.max")).and_then(|s| parse_memory_max(&s)),
            oom_kill: read_str(&self.root.join("memory.events"))
                .map(|s| parse_memory_events(&s))
                .unwrap_or(0),
            cpu_usage_usec: read_str(&self.root.join("cpu.stat"))
                .map(|s| parse_cpu_stat(&s))
                .unwrap_or(0),
        }
    }
}

impl Default for CgroupV2Reader {
    fn default() -> Self {
        Self::new()
    }
}

fn read_str(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn read_u64(path: &Path) -> Option<u64> {
    read_str(path).and_then(|s| s.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parses_memory_max() {
        assert_eq!(parse_memory_max("max"), None);
        assert_eq!(parse_memory_max(" 8589934592 \n"), Some(8_589_934_592));
        assert_eq!(parse_memory_max("garbage"), None);
    }

    #[test]
    fn parses_memory_events_oom_kill() {
        let events = "low 0\nhigh 0\nmax 0\noom 3\noom_kill 3\n";
        assert_eq!(parse_memory_events(events), 3);
        assert_eq!(parse_memory_events("oom 1\n"), 0);
    }

    #[test]
    fn parses_cpu_stat_usage() {
        let stat = "usage_usec 123456789\nuser_usec 100000\nsystem_usec 50000\n";
        assert_eq!(parse_cpu_stat(stat), 123_456_789);
        assert_eq!(parse_cpu_stat("user_usec 1\n"), 0);
    }

    fn fixture_root() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("argus-cgroup-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("memory.current"), "1073741824\n").unwrap();
        std::fs::write(root.join("memory.max"), "8589934592\n").unwrap();
        std::fs::write(root.join("memory.events"), "low 0\nhigh 0\nmax 0\noom 3\noom_kill 3\n").unwrap();
        std::fs::write(root.join("cpu.stat"), "usage_usec 123456789\n").unwrap();
        root
    }

    #[test]
    fn snapshot_reads_all_fields() {
        let root = fixture_root();
        let snap = CgroupV2Reader::with_root(&root).snapshot();
        assert_eq!(snap.memory_current_bytes, 1_073_741_824);
        assert_eq!(snap.memory_max_bytes, Some(8_589_934_592));
        assert_eq!(snap.oom_kill, 3);
        assert_eq!(snap.cpu_usage_usec, 123_456_789);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_tolerates_missing_files() {
        let root = PathBuf::from("/nonexistent/cgroup/root");
        let snap = CgroupV2Reader::with_root(&root).snapshot();
        assert_eq!(snap, CgroupSnapshot::default());
    }
}
