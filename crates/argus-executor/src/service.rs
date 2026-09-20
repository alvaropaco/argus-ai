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
}

/// A deterministic in-memory controller for tests.
#[derive(Debug, Default)]
pub struct MockServiceController {
    pub calls: std::sync::Mutex<Vec<(String, String)>>,
}

impl MockServiceController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn recorded_calls(&self) -> Vec<(String, String)> {
        self.calls.lock().unwrap().clone()
    }
}

impl ServiceController for MockServiceController {
    fn restart(&self, unit: &str) -> Result<(), ServiceError> {
        self.calls
            .lock()
            .unwrap()
            .push(("restart".into(), unit.into()));
        Ok(())
    }

    fn stop(&self, unit: &str) -> Result<(), ServiceError> {
        self.calls
            .lock()
            .unwrap()
            .push(("stop".into(), unit.into()));
        Ok(())
    }

    fn start(&self, unit: &str) -> Result<(), ServiceError> {
        self.calls
            .lock()
            .unwrap()
            .push(("start".into(), unit.into()));
        Ok(())
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
}
