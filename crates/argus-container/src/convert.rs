//! Conversion of container state into domain observations (CAP-3).

use chrono::{DateTime, Utc};
use uuid::Uuid;

use argus_domain::{ObservedValue, Observation, Provenance, ResourceId};

use crate::model::Container;

/// Emit observations for one container: state, restart count, and running flag
/// under subject `container:<id>`.
pub fn container_observations(container: &Container, now: DateTime<Utc>) -> Vec<Observation> {
    let subject = ResourceId::new("container", &container.id)
        .expect("a container id is a valid resource identifier");
    vec![
        text_obs(&subject, "container.state", &container.state, now),
        number_obs(
            &subject,
            "container.restart_count",
            container.restart_count as f64,
            now,
        ),
        bool_obs(&subject, "container.running", container.is_running(), now),
    ]
}

fn text_obs(subject: &ResourceId, attribute: &str, value: &str, now: DateTime<Utc>) -> Observation {
    Observation::new(
        Uuid::new_v4(),
        "argusd",
        subject.clone(),
        attribute,
        ObservedValue::Text(value.to_string()),
        1.0,
        Provenance::new("docker", "containers_json", now),
        now,
    )
    .expect("confidence 1.0 is valid")
}

fn number_obs(subject: &ResourceId, attribute: &str, value: f64, now: DateTime<Utc>) -> Observation {
    Observation::new(
        Uuid::new_v4(),
        "argusd",
        subject.clone(),
        attribute,
        ObservedValue::Number(value),
        1.0,
        Provenance::new("docker", "containers_json", now),
        now,
    )
    .expect("confidence 1.0 is valid")
}

fn bool_obs(subject: &ResourceId, attribute: &str, value: bool, now: DateTime<Utc>) -> Observation {
    Observation::new(
        Uuid::new_v4(),
        "argusd",
        subject.clone(),
        attribute,
        ObservedValue::Bool(value),
        1.0,
        Provenance::new("docker", "containers_json", now),
        now,
    )
    .expect("confidence 1.0 is valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn emits_container_subject_and_fields() {
        let now = Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap();
        let c = Container {
            id: "8dfafdbc3a40".to_string(),
            names: vec!["/checkout-api".to_string()],
            image: "registry/checkout-api:184".to_string(),
            state: "running".to_string(),
            status: "Up 3 minutes".to_string(),
            restart_count: 12,
        };
        let obs = container_observations(&c, now);
        assert_eq!(obs.len(), 3);
        for o in &obs {
            assert_eq!(o.subject().kind(), "container");
            assert_eq!(o.subject().identifier(), "8dfafdbc3a40");
        }
        let state = obs.iter().find(|o| o.attribute() == "container.state").unwrap();
        assert_eq!(state.value(), &ObservedValue::Text("running".to_string()));
        let restarts = obs.iter().find(|o| o.attribute() == "container.restart_count").unwrap();
        assert_eq!(restarts.value(), &ObservedValue::Number(12.0));
    }
}
