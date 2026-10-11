//! Live systemd client over the system D-Bus (zbus).
//!
//! This module is compile-tested on non-Linux hosts but its runtime behavior
//! requires systemd (Linux-gated). When the system bus or systemd is absent,
//! [`SystemdClient::connect`] returns [`SystemdError::Unavailable`] and the
//! daemon degrades gracefully.

use zbus::{Connection, proxy, zvariant::OwnedObjectPath};

use crate::SystemdError;
use crate::model::SystemdUnit;

/// The `org.freedesktop.systemd1.Manager` subset ARGUS uses.
#[proxy(
    interface = "org.freedesktop.systemd1.Manager",
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1"
)]
trait Manager {
    /// `ListUnits()` returns `(name, description, load_state, active_state,
    /// sub_state, following, unit_path, job_id, job_type, job_path)` per unit.
    fn list_units(
        &self,
    ) -> zbus::Result<
        Vec<(
            String,
            String,
            String,
            String,
            String,
            String,
            OwnedObjectPath,
            u32,
            String,
            OwnedObjectPath,
        )>,
    >;
}

/// The `org.freedesktop.journal1` subset ARGUS uses (spec 012): the age-based
/// journald vacuum. `Vacuum(since_usec)` removes every journal entry older
/// than the given epoch-microsecond cutoff and answers how many bytes it
/// freed. There is deliberately no size-based method: the interface's
/// semantics are age-based (spec 012 out-of-scope notes).
#[proxy(
    interface = "org.freedesktop.journal1",
    default_service = "org.freedesktop.journal1",
    default_path = "/org/freedesktop/journal1"
)]
trait Journal1 {
    /// `Vacuum(since_usec)` → bytes freed.
    fn vacuum(&self, since_usec: u64) -> zbus::Result<u64>;
}

/// A live systemd client over the system bus.
#[derive(Debug, Clone)]
pub struct SystemdClient {
    connection: Connection,
}

impl SystemdClient {
    /// Connect to the system bus. Fails with `Unavailable` when no system bus
    /// (or systemd) is reachable.
    pub async fn connect() -> Result<Self, SystemdError> {
        let connection = Connection::system()
            .await
            .map_err(|e| SystemdError::Unavailable(e.to_string()))?;
        Ok(Self { connection })
    }

    /// Snapshot every unit's state.
    pub async fn list_units(&self) -> Result<Vec<SystemdUnit>, SystemdError> {
        let proxy = ManagerProxy::new(&self.connection)
            .await
            .map_err(|e| SystemdError::Other(e.to_string()))?;
        let units = proxy
            .list_units()
            .await
            .map_err(|e| SystemdError::Other(e.to_string()))?;
        Ok(units.into_iter().map(normalize).collect())
    }

    /// Vacuums the journal: removes every journald entry older than
    /// `since_usec` (epoch microseconds), answering the bytes freed (spec 012
    /// FR-002). A missing journal1 service (no systemd, or journald not on the
    /// system bus) surfaces as a typed [`SystemdError`] — the caller fails
    /// closed, never silently.
    pub async fn vacuum_journal(&self, since_usec: u64) -> Result<u64, SystemdError> {
        let proxy = Journal1Proxy::new(&self.connection)
            .await
            .map_err(|e| SystemdError::Other(e.to_string()))?;
        proxy
            .vacuum(since_usec)
            .await
            .map_err(|e| SystemdError::Other(e.to_string()))
    }
}

/// Map a `ListUnits` tuple to a [`SystemdUnit`].
fn normalize(
    unit: (
        String,
        String,
        String,
        String,
        String,
        String,
        OwnedObjectPath,
        u32,
        String,
        OwnedObjectPath,
    ),
) -> SystemdUnit {
    SystemdUnit {
        name: unit.0,
        load_state: unit.2,
        active_state: unit.3,
        sub_state: unit.4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_maps_the_list_units_tuple() {
        let path =
            OwnedObjectPath::try_from("/org/freedesktop/systemd1/unit/nginx_2eservice").unwrap();
        let tuple = (
            "nginx.service".to_string(),                 // name
            "A high performance web server".to_string(), // description
            "loaded".to_string(),                        // load_state
            "active".to_string(),                        // active_state
            "running".to_string(),                       // sub_state
            String::new(),                               // following
            path.clone(),                                // unit_path
            0,                                           // job_id
            String::new(),                               // job_type
            OwnedObjectPath::try_from("/").unwrap(),     // job_path
        );

        let unit = normalize(tuple);
        assert_eq!(unit.name, "nginx.service");
        assert_eq!(unit.load_state, "loaded");
        assert_eq!(unit.active_state, "active");
        assert_eq!(unit.sub_state, "running");
        assert!(unit.is_active());
    }
}
