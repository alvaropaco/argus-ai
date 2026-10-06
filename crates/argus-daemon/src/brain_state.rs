//! Shared brain state: what the running brain has last seen, decided, and
//! done (spec 006 FR-001), and the operator's live control levers (FR-002).
//!
//! The brain loop writes its cycle record here; the cloud supervisor reads
//! it into the `sentinel.report`. The managed-configuration path writes the
//! control levers; the loop reads them on every tick — an operator's
//! autonomy change takes effect on the next cycle, no restart.

use std::sync::RwLock;

use argus_domain::AutonomyMode;

/// The last brain cycle, as the cloud should see it. Serializes into the
/// `sentinel.report`'s `last_cycle` field.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BrainCycleRecord {
    pub evidence: Vec<String>,
    pub provider_available: bool,
    pub decision: Option<String>,
    pub plan: Option<String>,
    pub outcome: Option<String>,
    pub at: chrono::DateTime<chrono::Utc>,
}

/// The brain's shared state: the last cycle record (read by the report,
/// written by the loop).
#[derive(Debug, Default)]
pub struct BrainState {
    last_cycle: RwLock<Option<BrainCycleRecord>>,
}

impl BrainState {
    pub fn record(&self, record: BrainCycleRecord) {
        *self
            .last_cycle
            .write()
            .expect("brain state is not poisoned") = Some(record);
    }

    pub fn last_cycle(&self) -> Option<BrainCycleRecord> {
        self.last_cycle
            .read()
            .expect("brain state is not poisoned")
            .clone()
    }
}

/// The brain's live control levers (spec 006 FR-002). Defaults mirror
/// `BrainConfig::default`; the managed-configuration path updates them and
/// the loop reads them per tick.
#[derive(Debug, Clone)]
pub struct BrainControl {
    pub autonomy: AutonomyMode,
    pub confidence_threshold: f64,
    pub interval_seconds: u64,
}

impl Default for BrainControl {
    fn default() -> Self {
        Self {
            autonomy: AutonomyMode::L0Observe,
            confidence_threshold: 0.3,
            interval_seconds: 60,
        }
    }
}

/// The shared handle the loop and the configuration path both hold.
#[derive(Debug, Default)]
pub struct BrainControlHandle(RwLock<BrainControl>);

impl BrainControlHandle {
    pub fn new(control: BrainControl) -> Self {
        Self(RwLock::new(control))
    }

    pub fn get(&self) -> BrainControl {
        self.0
            .read()
            .expect("brain control is not poisoned")
            .clone()
    }

    /// Atomically replaces the levers (used by the managed-configuration
    /// path after validating the whole delivered set).
    pub fn replace(&self, control: BrainControl) {
        *self.0.write().expect("brain control is not poisoned") = control;
    }

    /// Applies one dotted managed setting (`brain.autonomy` etc.). Unknown
    /// keys are ignored (they belong to another consumer); invalid values
    /// are rejected with the reason so the config result tells the truth.
    pub fn apply_setting(&self, key: &str, value: &serde_json::Value) -> Result<(), String> {
        let mut control = self.get();
        apply_to(&mut control, key, value)?;
        self.replace(control);
        Ok(())
    }

    /// Validates a whole set of `brain.*` settings without committing: the
    /// managed-configuration path validates first, then writes the store,
    /// then commits — a partial delivery must never half-apply.
    pub fn validate_all(
        &self,
        settings: &toml::Table,
    ) -> Result<Vec<(String, serde_json::Value)>, String> {
        let mut probe = self.get();
        let mut applied = Vec::new();
        for (key, value) in settings {
            if !key.starts_with("brain.") {
                continue;
            }
            let json = serde_json::to_value(value)
                .map_err(|e| format!("'{key}' is not representable: {e}"))?;
            apply_to(&mut probe, key, &json)?;
            applied.push((key.clone(), json));
        }
        Ok(applied)
    }

    /// Commits an already-validated set (companion to [`Self::validate_all`]).
    pub fn commit_all(&self, applied: &[(String, serde_json::Value)]) {
        let mut control = self.get();
        for (key, value) in applied {
            if let Err(reason) = apply_to(&mut control, key, value) {
                tracing::warn!(key = %key, reason = %reason, "validated setting failed to commit");
            }
        }
        self.replace(control);
    }
}

/// The single place a `brain.*` managed setting is interpreted.
fn apply_to(
    control: &mut BrainControl,
    key: &str,
    value: &serde_json::Value,
) -> Result<(), String> {
    match key {
        "brain.autonomy" => {
            let raw = value.as_str().ok_or("brain.autonomy must be a string")?;
            let autonomy: AutonomyMode = serde_json::from_value(serde_json::json!(raw)).map_err(
                |_| {
                    format!(
                        "brain.autonomy '{raw}' is not a level: l0_observe..l5_adaptive (or the legacy observe_only/propose/assisted)"
                    )
                },
            )?;
            control.autonomy = autonomy;
            Ok(())
        }
        "brain.confidence_threshold" => {
            let raw = value
                .as_f64()
                .ok_or("brain.confidence_threshold must be a number")?;
            if !(0.0..=1.0).contains(&raw) {
                return Err(format!("brain.confidence_threshold {raw} outside [0, 1]"));
            }
            control.confidence_threshold = raw;
            Ok(())
        }
        "brain.interval_seconds" => {
            let raw = value
                .as_u64()
                .ok_or("brain.interval_seconds must be an integer")?;
            if raw < 5 {
                return Err(format!("brain.interval_seconds {raw} below the 5s floor"));
            }
            control.interval_seconds = raw;
            Ok(())
        }
        _ => Ok(()), // not ours; another consumer may own it
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_loop_records_and_the_report_reads() {
        let state = BrainState::default();
        assert!(state.last_cycle().is_none());
        state.record(BrainCycleRecord {
            evidence: vec!["unit: failed".into()],
            provider_available: true,
            decision: Some("decided".into()),
            plan: Some("restore".into()),
            outcome: Some("Completed".into()),
            at: chrono::Utc::now(),
        });
        let record = state.last_cycle().unwrap();
        assert_eq!(record.plan.as_deref(), Some("restore"));
    }

    #[test]
    fn autonomy_settings_apply_and_reject() {
        let handle = BrainControlHandle::default();
        assert_eq!(handle.get().autonomy, AutonomyMode::L0Observe);

        handle
            .apply_setting("brain.autonomy", &json!("l3_assisted"))
            .unwrap();
        assert_eq!(handle.get().autonomy, AutonomyMode::L3Assisted);
        // Legacy names work too (same serde path as the config file).
        handle
            .apply_setting("brain.autonomy", &json!("propose"))
            .unwrap();
        assert_eq!(handle.get().autonomy, AutonomyMode::L2Recommend);

        assert!(
            handle
                .apply_setting("brain.autonomy", &json!("rampage"))
                .is_err()
        );
        assert_eq!(handle.get().autonomy, AutonomyMode::L2Recommend);
    }

    #[test]
    fn threshold_and_interval_are_clamped_by_validation_not_silently() {
        let handle = BrainControlHandle::default();
        assert!(
            handle
                .apply_setting("brain.confidence_threshold", &json!(1.5))
                .is_err()
        );
        assert!(
            handle
                .apply_setting("brain.interval_seconds", &json!(1))
                .is_err()
        );
        handle
            .apply_setting("brain.confidence_threshold", &json!(0.7))
            .unwrap();
        handle
            .apply_setting("brain.interval_seconds", &json!(30))
            .unwrap();
        assert_eq!(handle.get().confidence_threshold, 0.7);
        assert_eq!(handle.get().interval_seconds, 30);
        // Not-our keys are ignored, never rejected.
        assert!(
            handle
                .apply_setting("model.provider", &json!("deepseek"))
                .is_ok()
        );
    }
}
