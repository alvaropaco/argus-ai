//! The continuous observation loop (Milestone 1).
//!
//! Drives the kernel-native sensors, the process inventory, and the
//! systemd/container adapters on a schedule; persists observations, feeds
//! baselines, and publishes events. The deterministic processing — event
//! construction and baseline deviation — is pure and unit-tested; the
//! collection I/O is Linux-gated (ADR-0032).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use argus_anomaly::{detect_deviation, detect_restart_loops, Baseline, DeviationConfig, RestartLoop, UnitState};
use argus_container::{container_observations, DockerClient};
use argus_domain::{DomainEvent, EventType, Observation, ResourceId, Severity};
use argus_events::{EventBus, LocalEventBus};
use argus_observe::{
    AnomalyConfig, LifecycleChange, ObservationEmitter, Observer, ProcessAnomaly, ProcSnapshotter,
};
use argus_state::DomainRepository;
use argus_systemd::{unit_observations, SystemdClient};

/// Default interval between observation ticks.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(5);
/// Restart-count delta in one interval above which a container is flagged as
/// restart-looping.
const RESTART_LOOP_THRESHOLD: u32 = 3;

/// Emit lifecycle events for a tick's process changes.
pub fn lifecycle_events(changes: &[LifecycleChange], now: DateTime<Utc>) -> Vec<DomainEvent> {
    changes
        .iter()
        .map(|change| match change {
            LifecycleChange::Started { pid, name, ppid } => event(
                "process.started",
                &format!("process:{pid}"),
                Severity::Info,
                serde_json::json!({ "name": name, "ppid": ppid }),
                now,
            ),
            LifecycleChange::Exited { pid, name } => event(
                "process.exited",
                &format!("process:{pid}"),
                Severity::Info,
                serde_json::json!({ "name": name }),
                now,
            ),
            LifecycleChange::Changed { pid, name, fields } => event(
                "process.changed",
                &format!("process:{pid}"),
                Severity::Info,
                serde_json::json!({
                    "name": name,
                    "fields": fields.iter().map(|f| format!("{f:?}")).collect::<Vec<_>>()
                }),
                now,
            ),
        })
        .collect()
}

/// Emit anomaly events for a tick's process anomalies.
pub fn anomaly_events(
    anomalies: &[ProcessAnomaly],
    host: &ResourceId,
    now: DateTime<Utc>,
) -> Vec<DomainEvent> {
    anomalies
        .iter()
        .map(|a| {
            let subject = a
                .pid
                .map(|pid| format!("process:{pid}"))
                .unwrap_or_else(|| host.as_str().to_string());
            event(
                "process.anomaly",
                &subject,
                Severity::Warning,
                serde_json::json!({ "kind": format!("{:?}", a.kind), "detail": a.detail }),
                now,
            )
        })
        .collect()
}

/// Emit restart-loop events under `subject_kind` (`service` or `container`).
pub fn restart_loop_events(
    loops: &[RestartLoop],
    subject_kind: &str,
    now: DateTime<Utc>,
) -> Vec<DomainEvent> {
    loops
        .iter()
        .map(|l| {
            event(
                "unit.restart.loop",
                &format!("{subject_kind}:{}", l.name),
                Severity::Warning,
                serde_json::json!({ "restarts": l.restarts }),
                now,
            )
        })
        .collect()
}

fn event(
    event_type: &str,
    subject: &str,
    severity: Severity,
    payload: serde_json::Value,
    now: DateTime<Utc>,
) -> DomainEvent {
    DomainEvent::new(
        Uuid::new_v4(),
        EventType::new(event_type).expect("valid event type"),
        now,
        "argusd",
        subject,
        severity,
        None,
        None,
        payload,
    )
}

/// Maintains rolling baselines per signal and emits deviation events.
#[derive(Debug)]
pub struct BaselineManager {
    baselines: HashMap<String, Baseline>,
    capacity: usize,
    min_samples: usize,
    config: DeviationConfig,
}

impl BaselineManager {
    pub fn new(capacity: usize, min_samples: usize, config: DeviationConfig) -> Self {
        Self {
            baselines: HashMap::new(),
            capacity,
            min_samples,
            config,
        }
    }

    /// Observe one tick's signals, returning the baseline events (established +
    /// deviation) that resulted.
    pub fn observe(
        &mut self,
        signals: &[(String, f64)],
        host: &ResourceId,
        now: DateTime<Utc>,
    ) -> Vec<DomainEvent> {
        let mut events = Vec::new();
        for (name, value) in signals {
            let baseline = self
                .baselines
                .entry(name.clone())
                .or_insert_with(|| Baseline::new(self.capacity, self.min_samples));
            let was_ready = baseline.is_ready();
            // Detect deviation against the *previous* window, then fold the new
            // sample in.
            let deviation = detect_deviation(name, *value, baseline, &self.config);
            baseline.observe(*value);

            if let Some(dev) = deviation {
                events.push(event(
                    "baseline.deviation",
                    host.as_str(),
                    Severity::Warning,
                    serde_json::json!({
                        "signal": name,
                        "value": dev.value,
                        "z_score": dev.z_score,
                        "kind": format!("{:?}", dev.kind),
                    }),
                    now,
                ));
            } else if !was_ready && baseline.is_ready() {
                events.push(event(
                    "baseline.established",
                    host.as_str(),
                    Severity::Info,
                    serde_json::json!({ "signal": name }),
                    now,
                ));
            }
        }
        events
    }
}

/// The continuous observation loop.
pub struct ObservationLoop {
    repository: Arc<dyn DomainRepository>,
    events: Arc<LocalEventBus>,
    host: ResourceId,
    interval: Duration,
    emitter: ObservationEmitter,
    observer: Observer,
    baselines: BaselineManager,
    systemd: Option<SystemdClient>,
    docker: Option<DockerClient>,
    prev_cpu: Option<argus_sensors::stat::CpuTimes>,
    prev_containers: HashMap<String, UnitState>,
}

impl ObservationLoop {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repository: Arc<dyn DomainRepository>,
        events: Arc<LocalEventBus>,
        host: ResourceId,
        snapshotter: ProcSnapshotter,
        anomaly_config: AnomalyConfig,
    ) -> Self {
        Self {
            repository,
            events,
            host: host.clone(),
            interval: DEFAULT_INTERVAL,
            emitter: ObservationEmitter::new(host),
            observer: Observer::new(snapshotter, anomaly_config),
            baselines: BaselineManager::new(300, 30, DeviationConfig::default()),
            systemd: None,
            docker: None,
            prev_cpu: None,
            prev_containers: HashMap::new(),
        }
    }

    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Run forever. Connects the optional adapters once (degrading gracefully)
    /// and ticks on a fixed interval.
    pub async fn run(mut self) {
        self.systemd = SystemdClient::connect().await.ok();
        self.docker = Some(DockerClient::new());

        let mut ticker = tokio::time::interval(self.interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            if let Err(error) = self.tick().await {
                tracing::warn!(%error, "observation tick failed");
            }
        }
    }

    async fn tick(&mut self) -> Result<(), String> {
        let now = Utc::now();

        let (observations, events) = self.collect_host(now)?;
        self.persist_and_publish(&observations, &events).await?;

        self.tick_systemd(now).await;
        self.tick_docker(now).await;
        Ok(())
    }

    /// Synchronous host collection: read sensors + inventory, convert, and
    /// compute baseline events. Returns observations to persist and events to
    /// publish.
    fn collect_host(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<(Vec<Observation>, Vec<DomainEvent>), String> {
        let mem = argus_sensors::meminfo::read().map_err(|e| e.to_string())?;
        let load = argus_sensors::loadavg::read().map_err(|e| e.to_string())?;
        let cpu = argus_sensors::stat::read().map_err(|e| e.to_string())?;
        let vm = argus_sensors::vmstat::read().map_err(|e| e.to_string())?;

        let tick = self.observer.tick().map_err(|e| e.to_string())?;

        let mut observations = Vec::new();
        observations.extend(self.emitter.memory(now, &mem));
        observations.extend(self.emitter.load(now, &load));
        observations.extend(self.emitter.cpu(now, &cpu));
        observations.extend(self.emitter.vm(now, &vm));
        for rec in tick.inventory.processes.values() {
            observations.extend(self.emitter.process(now, rec));
        }

        let mut signals: Vec<(String, f64)> = vec![
            ("memory.used_percent".to_string(), mem.used_percent()),
            ("load.1m".to_string(), load.load1),
        ];
        if let Some(prev) = self.prev_cpu {
            signals.push(("cpu.utilization".to_string(), cpu.utilization_since(&prev)));
        }
        self.prev_cpu = Some(cpu);

        let mut events = lifecycle_events(&tick.changes, now);
        events.extend(anomaly_events(&tick.anomalies, &self.host, now));
        events.extend(self.baselines.observe(&signals, &self.host, now));

        Ok((observations, events))
    }

    async fn tick_systemd(&mut self, now: DateTime<Utc>) {
        let Some(client) = &self.systemd else { return };
        let Ok(units) = client.list_units().await else { return };

        let mut observations = Vec::new();
        let mut events = Vec::new();
        for unit in &units {
            observations.extend(unit_observations(unit, now));
            if unit.is_restart_looping() {
                events.push(event(
                    "unit.restart.loop",
                    &format!("service:{}", unit.name),
                    Severity::Warning,
                    serde_json::json!({}),
                    now,
                ));
            }
        }
        self.persist_and_publish(&observations, &events).await.ok();
    }

    async fn tick_docker(&mut self, now: DateTime<Utc>) {
        let Some(client) = &self.docker else { return };
        let Ok(containers) = client.list_containers().await else { return };

        let mut observations = Vec::new();
        let mut current: HashMap<String, UnitState> = HashMap::new();
        for container in &containers {
            observations.extend(container_observations(container, now));
            let name = container
                .primary_name()
                .unwrap_or(&container.id)
                .to_string();
            current.insert(
                container.id.clone(),
                UnitState {
                    name,
                    restart_count: container.restart_count,
                    active: container.is_running(),
                },
            );
        }

        let loops = detect_restart_loops(&self.prev_containers, &current, RESTART_LOOP_THRESHOLD);
        self.prev_containers = current;
        let events = restart_loop_events(&loops, "container", now);

        self.persist_and_publish(&observations, &events).await.ok();
    }

    async fn persist_and_publish(
        &self,
        observations: &[Observation],
        events: &[DomainEvent],
    ) -> Result<(), String> {
        for observation in observations {
            self.repository
                .put_observation(observation)
                .await
                .map_err(|e| e.to_string())?;
        }
        for event in events {
            self.events.publish(event).await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use argus_observe::ProcessAnomalyKind;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap()
    }

    #[test]
    fn lifecycle_events_map_changes() {
        let changes = vec![
            LifecycleChange::Started {
                pid: 2,
                name: "worker".to_string(),
                ppid: 1,
            },
            LifecycleChange::Exited {
                pid: 3,
                name: "gone".to_string(),
            },
        ];
        let events = lifecycle_events(&changes, now());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_type().as_str(), "process.started");
        assert_eq!(events[0].subject(), "process:2");
        assert_eq!(events[1].event_type().as_str(), "process.exited");
        assert_eq!(events[1].subject(), "process:3");
        assert_eq!(events[0].source(), "argusd");
    }

    #[test]
    fn anomaly_events_use_host_for_global_and_process_for_specific() {
        let host = ResourceId::new("host", "local").unwrap();
        let anomalies = vec![
            ProcessAnomaly {
                kind: ProcessAnomalyKind::Explosion,
                pid: None,
                detail: "5 processes started".to_string(),
            },
            ProcessAnomaly {
                kind: ProcessAnomalyKind::Runaway,
                pid: Some(42),
                detail: "x".to_string(),
            },
        ];
        let events = anomaly_events(&anomalies, &host, now());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].subject(), "host:local");
        assert_eq!(events[1].subject(), "process:42");
        assert_eq!(events[0].severity(), Severity::Warning);
    }

    #[test]
    fn restart_loop_events_use_subject_kind() {
        let loops = vec![RestartLoop {
            name: "checkout-api".to_string(),
            restarts: 3,
        }];
        let events = restart_loop_events(&loops, "container", now());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type().as_str(), "unit.restart.loop");
        assert_eq!(events[0].subject(), "container:checkout-api");
    }

    #[test]
    fn baseline_manager_establishes_then_deviates() {
        let host = ResourceId::new("host", "local").unwrap();
        let now = now();
        let config = DeviationConfig {
            z_threshold: 3.0,
            hard_limit: None,
        };
        let mut mgr = BaselineManager::new(10, 2, config);
        let signal = [("cpu.utilization".to_string(), 50.0)];

        // One sample: not ready, no event.
        assert!(mgr.observe(&signal, &host, now).is_empty());

        // Second sample: baseline becomes ready → established.
        let events = mgr.observe(&[("cpu.utilization".to_string(), 52.0)], &host, now);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type().as_str(), "baseline.established");

        // A far-out sample → deviation.
        let events = mgr.observe(&[("cpu.utilization".to_string(), 200.0)], &host, now);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type().as_str(), "baseline.deviation");
        assert_eq!(events[0].subject(), "host:local");
    }
}
