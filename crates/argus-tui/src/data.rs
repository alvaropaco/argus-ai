//! Runtime data fetched from `argusd` for display.

use std::path::Path;

use anyhow::Context;
use argus_ipc::{Client, Operation};
use serde_json::{Value, json};
use uuid::Uuid;

/// Snapshot of the daemon state shown by the TUI.
pub struct Data {
    pub health: Value,
    pub status: Value,
    pub capabilities: Value,
    pub plugins: Value,
    pub config: Value,
}

impl Data {
    pub fn unavailable(message: &str) -> Self {
        let v = json!({ "error": message });
        Self {
            health: v.clone(),
            status: v.clone(),
            capabilities: v.clone(),
            plugins: v.clone(),
            config: v,
        }
    }
}

pub async fn fetch(socket_path: &Path) -> anyhow::Result<Data> {
    let mut client = Client::connect(socket_path)
        .await
        .with_context(|| format!("connect to argusd at {}", socket_path.display()))?;

    Ok(Data {
        health: request(&mut client, Operation::HealthGet).await?,
        status: request(&mut client, Operation::StatusGet).await?,
        capabilities: request(&mut client, Operation::CapabilitiesList).await?,
        plugins: request(&mut client, Operation::PluginsList).await?,
        config: request(&mut client, Operation::ConfigGet).await?,
    })
}

async fn request(client: &mut Client, operation: Operation) -> anyhow::Result<Value> {
    let response = client.request(operation, Uuid::new_v4(), json!({})).await?;
    if response.ok {
        Ok(response.result.unwrap_or(Value::Null))
    } else {
        let code = response.error.as_ref().map(|e| e.code);
        anyhow::bail!("IPC error ({code:?})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_contains_error_in_every_view() {
        let data = Data::unavailable("daemon unreachable");
        assert_eq!(data.health["error"], "daemon unreachable");
        assert_eq!(data.status["error"], "daemon unreachable");
        assert_eq!(data.capabilities["error"], "daemon unreachable");
        assert_eq!(data.plugins["error"], "daemon unreachable");
        assert_eq!(data.config["error"], "daemon unreachable");
    }
}
