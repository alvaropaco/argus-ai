//! Typed systemd unit state and its failure/restart-loop predicates (CAP-3).

/// The subset of a systemd unit's D-Bus state that ARGUS tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemdUnit {
    /// e.g. `nginx.service`.
    pub name: String,
    /// `loaded`, `not-found`, `error`, `masked`, …
    pub load_state: String,
    /// `active`, `inactive`, `failed`, `activating`, `deactivating`, …
    pub active_state: String,
    /// `running`, `dead`, `exited`, `failed`, `auto-restart`, …
    pub sub_state: String,
}

impl SystemdUnit {
    /// Whether the unit is currently active.
    pub fn is_active(&self) -> bool {
        self.active_state == "active"
    }

    /// Whether the unit is in a failed state.
    pub fn is_failed(&self) -> bool {
        self.active_state == "failed"
    }

    /// Whether the unit is in a restart loop. systemd sets `sub_state` to
    /// `auto-restart` while a unit is in its restart backoff.
    pub fn is_restart_looping(&self) -> bool {
        self.sub_state == "auto-restart"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(active: &str, sub: &str) -> SystemdUnit {
        SystemdUnit {
            name: "nginx.service".to_string(),
            load_state: "loaded".to_string(),
            active_state: active.to_string(),
            sub_state: sub.to_string(),
        }
    }

    #[test]
    fn active_running_unit() {
        let u = unit("active", "running");
        assert!(u.is_active());
        assert!(!u.is_failed());
        assert!(!u.is_restart_looping());
    }

    #[test]
    fn failed_unit() {
        let u = unit("failed", "failed");
        assert!(u.is_failed());
        assert!(!u.is_active());
    }

    #[test]
    fn restart_looping_unit() {
        let u = unit("activating", "auto-restart");
        assert!(u.is_restart_looping());
    }

    #[test]
    fn inactive_unit() {
        let u = unit("inactive", "dead");
        assert!(!u.is_active());
        assert!(!u.is_failed());
        assert!(!u.is_restart_looping());
    }
}
