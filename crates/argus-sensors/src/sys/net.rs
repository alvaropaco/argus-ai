//! `/sys/class/net` reader: per-interface operational state (up/down).
//!
//! The root is parameterized so tests use a fixture tree; production uses
//! `/sys/class/net`.

use std::path::Path;

use crate::SensorError;

/// Operational state of one network interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceState {
    pub name: String,
    /// `up`, `down`, `unknown`, `dormant`, `lowerlayerdown`, …
    pub oper_state: String,
}

impl InterfaceState {
    pub fn is_up(&self) -> bool {
        self.oper_state == "up"
    }
}

/// List interfaces and their operational state under `base`. Sorted by name.
pub fn read_interfaces(base: &Path) -> Result<Vec<InterfaceState>, SensorError> {
    let entries = std::fs::read_dir(base).map_err(|source| SensorError::Read {
        name: "host.network.interfaces",
        source,
    })?;

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().into_string().ok() else {
            continue;
        };
        let oper_state = std::fs::read_to_string(entry.path().join("operstate"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        out.push(InterfaceState { name, oper_state });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_root() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("argus-net-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(root.join("eth0")).unwrap();
        std::fs::create_dir_all(root.join("lo")).unwrap();
        std::fs::write(root.join("eth0/operstate"), "up\n").unwrap();
        std::fs::write(root.join("lo/operstate"), "down\n").unwrap();
        root
    }

    #[test]
    fn reads_interfaces_sorted() {
        let root = fixture_root();
        let ifaces = read_interfaces(&root).unwrap();
        assert_eq!(ifaces.len(), 2);
        assert_eq!(ifaces[0].name, "eth0");
        assert!(ifaces[0].is_up());
        assert_eq!(ifaces[1].name, "lo");
        assert!(!ifaces[1].is_up());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_base_is_an_error() {
        assert!(read_interfaces(Path::new("/nonexistent/net/class")).is_err());
    }
}
