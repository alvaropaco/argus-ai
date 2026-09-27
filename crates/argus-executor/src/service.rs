//! Host-service control executor (systemd units).

use std::process::Command;

/// Errors from service control.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("service control unavailable: {0}")]
    Unavailable(String),

    #[error("service operation failed: {0}")]
    Failed(String),
}

/// Controls host services (systemd units) backing the `host.service.*`
/// capabilities. A trait so tests use a deterministic mock.
pub trait ServiceController: Send + Sync {
    fn restart(&self, unit: &str) -> Result<(), ServiceError>;
    fn stop(&self, unit: &str) -> Result<(), ServiceError>;
    fn start(&self, unit: &str) -> Result<(), ServiceError>;

    /// Whether the unit is currently active (running), read from live state.
    ///
    /// This is the desired-state source for action idempotency: "already in the
    /// desired state" is decided against this read, not against the plan
    /// (ADR-0028 §5).
    fn is_active(&self, unit: &str) -> Result<bool, ServiceError>;
}

/// systemd-backed controller. Uses `systemctl` as the userspace fallback for
/// the structured D-Bus API (Principle 5).
#[derive(Debug, Default)]
pub struct SystemdServiceController;

impl SystemdServiceController {
    pub fn new() -> Self {
        Self
    }

    fn run(&self, verb: &str, unit: &str) -> Result<(), ServiceError> {
        let output = Command::new("systemctl")
            .arg(verb)
            .arg(unit)
            .output()
            .map_err(|e| ServiceError::Unavailable(e.to_string()))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(ServiceError::Failed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ))
        }
    }
}

impl ServiceController for SystemdServiceController {
    fn restart(&self, unit: &str) -> Result<(), ServiceError> {
        self.run("restart", unit)
    }

    fn stop(&self, unit: &str) -> Result<(), ServiceError> {
        self.run("stop", unit)
    }

    fn start(&self, unit: &str) -> Result<(), ServiceError> {
        self.run("start", unit)
    }

    fn is_active(&self, unit: &str) -> Result<bool, ServiceError> {
        // A structured single-value query (`ActiveState=`) rather than parsing
        // human-oriented CLI prose (Principle 5).
        let output = Command::new("systemctl")
            .arg("show")
            .arg(unit)
            .arg("--property=ActiveState")
            .arg("--value")
            .output()
            .map_err(|e| ServiceError::Unavailable(e.to_string()))?;
        if !output.status.success() {
            return Err(ServiceError::Failed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim() == "active")
    }
}

/// A deterministic in-memory controller for tests.
#[derive(Debug, Default)]
pub struct MockServiceController {
    pub calls: std::sync::Mutex<Vec<(String, String)>>,
    /// Units currently marked active, so tests can drive the desired-state path.
    pub active: std::sync::Mutex<std::collections::BTreeSet<String>>,
}

impl MockServiceController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn recorded_calls(&self) -> Vec<(String, String)> {
        self.calls.lock().unwrap().clone()
    }

    /// Sets whether `unit` is active, for the desired-state check.
    pub fn set_active(&self, unit: &str, active: bool) {
        let mut units = self.active.lock().unwrap();
        if active {
            units.insert(unit.to_string());
        } else {
            units.remove(unit);
        }
    }
}

impl ServiceController for MockServiceController {
    fn restart(&self, unit: &str) -> Result<(), ServiceError> {
        self.calls
            .lock()
            .unwrap()
            .push(("restart".into(), unit.into()));
        self.active.lock().unwrap().insert(unit.to_string());
        Ok(())
    }

    fn stop(&self, unit: &str) -> Result<(), ServiceError> {
        self.calls
            .lock()
            .unwrap()
            .push(("stop".into(), unit.into()));
        self.active.lock().unwrap().remove(unit);
        Ok(())
    }

    fn start(&self, unit: &str) -> Result<(), ServiceError> {
        self.calls
            .lock()
            .unwrap()
            .push(("start".into(), unit.into()));
        self.active.lock().unwrap().insert(unit.to_string());
        Ok(())
    }

    fn is_active(&self, unit: &str) -> Result<bool, ServiceError> {
        Ok(self.active.lock().unwrap().contains(unit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_records_calls() {
        let controller = MockServiceController::new();
        controller.restart("nginx.service").unwrap();
        controller.stop("nginx.service").unwrap();
        assert_eq!(
            controller.recorded_calls(),
            vec![
                ("restart".to_string(), "nginx.service".to_string()),
                ("stop".to_string(), "nginx.service".to_string()),
            ]
        );
    }

    #[test]
    fn systemd_controller_is_constructible() {
        let _ = SystemdServiceController::new();
    }

    #[test]
    fn mock_reports_active_state() {
        let controller = MockServiceController::new();
        assert!(!controller.is_active("nginx.service").unwrap());
        controller.set_active("nginx.service", true);
        assert!(controller.is_active("nginx.service").unwrap());
        controller.set_active("nginx.service", false);
        assert!(!controller.is_active("nginx.service").unwrap());
    }

    #[test]
    fn mock_start_stop_restart_reflect_active_state() {
        let controller = MockServiceController::new();
        controller.start("nginx.service").unwrap();
        assert!(controller.is_active("nginx.service").unwrap());
        controller.stop("nginx.service").unwrap();
        assert!(!controller.is_active("nginx.service").unwrap());
        controller.restart("nginx.service").unwrap();
        assert!(controller.is_active("nginx.service").unwrap());
    }
}
