//! Typed container state and its predicates (CAP-3).

use serde::Deserialize;

/// The subset of Docker's `/containers/json` entry that ARGUS tracks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Container {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub image: String,
    /// `running`, `restarting`, `exited`, `created`, `dead`, `paused`, …
    #[serde(default)]
    pub state: String,
    /// Human status string (`Up 3 minutes`, `Exited (1) 2 hours ago`, …).
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub restart_count: u32,
}

impl Container {
    /// The primary name with the leading `/` stripped (e.g. `checkout-api`).
    pub fn primary_name(&self) -> Option<&str> {
        self.names.first().map(|n| n.strip_prefix('/').unwrap_or(n))
    }

    pub fn is_running(&self) -> bool {
        self.state == "running"
    }

    pub fn is_restarting(&self) -> bool {
        self.state == "restarting"
    }

    pub fn is_exited(&self) -> bool {
        self.state == "exited"
    }
}

/// Parse a Docker `/containers/json` response body.
pub fn parse_containers(input: &str) -> Result<Vec<Container>, serde_json::Error> {
    serde_json::from_str(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"[
        {
            "Id": "8dfafdbc3a40",
            "Names": ["/checkout-api"],
            "Image": "registry/checkout-api:184",
            "State": "running",
            "Status": "Up 3 minutes",
            "RestartCount": 0
        },
        {
            "Id": "9ab7cdef0123",
            "Names": ["/payments-api"],
            "Image": "registry/payments-api:57",
            "State": "restarting",
            "Status": "Restarting (1) 2 seconds ago",
            "RestartCount": 12
        },
        {
            "Id": "1c2d3e4f5678",
            "Names": ["/retired-worker"],
            "Image": "registry/worker:3",
            "State": "exited",
            "Status": "Exited (1) 2 hours ago",
            "RestartCount": 1
        }
    ]"#;

    #[test]
    fn parses_containers() {
        let containers = parse_containers(FIXTURE).unwrap();
        assert_eq!(containers.len(), 3);

        let running = &containers[0];
        assert_eq!(running.id, "8dfafdbc3a40");
        assert_eq!(running.primary_name(), Some("checkout-api"));
        assert_eq!(running.image, "registry/checkout-api:184");
        assert!(running.is_running());
        assert!(!running.is_restarting());

        let restarting = &containers[1];
        assert!(restarting.is_restarting());
        assert_eq!(restarting.restart_count, 12);

        let exited = &containers[2];
        assert!(exited.is_exited());
    }

    #[test]
    fn tolerates_missing_fields() {
        let containers = parse_containers(r#"[{"Id": "abc123"}]"#).unwrap();
        assert_eq!(containers.len(), 1);
        assert_eq!(containers[0].id, "abc123");
        assert_eq!(containers[0].restart_count, 0);
        assert!(containers[0].primary_name().is_none());
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(parse_containers("not json").is_err());
    }
}
