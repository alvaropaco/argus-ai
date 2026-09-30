//! Restart-loop detection over generic managed units (CAP-3/CAP-7).
//!
//! A "unit" is a long-running operational unit — a systemd service, a
//! container, or a Kubernetes workload all have a restart counter and an active
//! flag (domain-model §3.10). The systemd and container adapters populate
//! [`UnitState`] snapshots; this module detects loops between snapshots.

use std::collections::HashMap;

/// The restart/activity state of one managed unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitState {
    pub name: String,
    pub restart_count: u32,
    pub active: bool,
}

/// A unit that restarted `threshold` or more times within one interval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestartLoop {
    pub name: String,
    pub restarts: u32,
}

/// Compare two snapshots and flag units whose restart count grew by at least
/// `threshold`. New units and counter resets never read as a loop. Output is
/// sorted by name for determinism.
pub fn detect_restart_loops(
    prev: &HashMap<String, UnitState>,
    curr: &HashMap<String, UnitState>,
    threshold: u32,
) -> Vec<RestartLoop> {
    let mut out: Vec<RestartLoop> = Vec::new();
    for (name, current) in curr {
        let delta = prev
            .get(name)
            .map(|p| current.restart_count.saturating_sub(p.restart_count))
            .unwrap_or(0);
        if delta >= threshold {
            out.push(RestartLoop {
                name: name.clone(),
                restarts: delta,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(name: &str, restarts: u32, active: bool) -> UnitState {
        UnitState {
            name: name.to_string(),
            restart_count: restarts,
            active,
        }
    }

    fn snap(units: &[UnitState]) -> HashMap<String, UnitState> {
        units.iter().cloned().map(|u| (u.name.clone(), u)).collect()
    }

    #[test]
    fn detects_a_restart_loop() {
        let prev = snap(&[unit("checkout-api", 1, true)]);
        let curr = snap(&[unit("checkout-api", 3, true)]);
        let loops = detect_restart_loops(&prev, &curr, 2);
        assert_eq!(
            loops,
            vec![RestartLoop {
                name: "checkout-api".to_string(),
                restarts: 2,
            }]
        );
    }

    #[test]
    fn new_unit_is_not_a_loop() {
        let prev = snap(&[unit("a", 0, true)]);
        let curr = snap(&[unit("a", 0, true), unit("b", 5, true)]);
        assert!(detect_restart_loops(&prev, &curr, 1).is_empty());
    }

    #[test]
    fn below_threshold_is_not_a_loop() {
        let prev = snap(&[unit("a", 1, true)]);
        let curr = snap(&[unit("a", 2, true)]);
        assert!(detect_restart_loops(&prev, &curr, 2).is_empty());
    }

    #[test]
    fn counter_reset_is_not_a_loop() {
        let prev = snap(&[unit("a", 5, true)]);
        let curr = snap(&[unit("a", 1, true)]); // reset (daemon restarted)
        assert!(detect_restart_loops(&prev, &curr, 1).is_empty());
    }

    #[test]
    fn output_is_sorted_by_name() {
        let prev = snap(&[unit("z", 0, true), unit("a", 0, true)]);
        let curr = snap(&[unit("z", 3, true), unit("a", 3, true)]);
        let loops = detect_restart_loops(&prev, &curr, 1);
        assert_eq!(loops[0].name, "a");
        assert_eq!(loops[1].name, "z");
    }
}
