//! Conversion of systemd unit state into domain observations (CAP-3).

use chrono::{DateTime, Utc};
use uuid::Uuid;

use argus_domain::{Observation, ObservedValue, Provenance, ResourceId};

use crate::model::SystemdUnit;

/// Emit observations for one unit: active/failed/restart-looping booleans under
/// subject `service:<unit>`.
pub fn unit_observations(unit: &SystemdUnit, now: DateTime<Utc>) -> Vec<Observation> {
    let subject = ResourceId::new("service", &unit.name)
        .expect("a systemd unit name is a valid resource identifier");
    vec![
        bool_obs(&subject, "service.active", unit.is_active(), now),
        bool_obs(&subject, "service.failed", unit.is_failed(), now),
        bool_obs(
            &subject,
            "service.restart_looping",
            unit.is_restart_looping(),
            now,
        ),
    ]
}

fn bool_obs(subject: &ResourceId, attribute: &str, value: bool, now: DateTime<Utc>) -> Observation {
    Observation::new(
        Uuid::new_v4(),
        "argusd",
        subject.clone(),
        attribute,
        ObservedValue::Bool(value),
        1.0,
        Provenance::new("systemd", "list_units", now),
        now,
    )
    .expect("confidence 1.0 is valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn unit(active: &str, sub: &str) -> SystemdUnit {
        SystemdUnit {
            name: "nginx.service".to_string(),
            load_state: "loaded".to_string(),
            active_state: active.to_string(),
            sub_state: sub.to_string(),
        }
    }

    #[test]
    fn emits_service_subject_and_flags() {
        let now = Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap();
        let obs = unit_observations(&unit("failed", "failed"), now);
        assert_eq!(obs.len(), 3);
        for o in &obs {
            assert_eq!(o.subject().kind(), "service");
            assert_eq!(o.subject().identifier(), "nginx.service");
        }
        let active = obs
            .iter()
            .find(|o| o.attribute() == "service.active")
            .unwrap();
        assert_eq!(active.value(), &ObservedValue::Bool(false));
        let failed = obs
            .iter()
            .find(|o| o.attribute() == "service.failed")
            .unwrap();
        assert_eq!(failed.value(), &ObservedValue::Bool(true));
    }
}
