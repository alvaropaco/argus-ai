//! Observation conversion: the seam where sensor and inventory data become
//! domain evidence (ADR-0032 §2). Everything here is pure and deterministic —
//! the host identity and collection timestamp are injected, so it is testable
//! without a host or a clock.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use argus_domain::{ObservedValue, Observation, Provenance, ResourceId};

use crate::inventory::ProcessRecord;

/// Emits deterministic [`Observation`]s for host metrics and process inventory.
///
/// `source` is `argusd` (the daemon is the collector); the provenance `source`
/// names the kernel interface and `method` names the reader, matching the
/// convention already used by `argus-daemon`.
#[derive(Debug, Clone)]
pub struct ObservationEmitter {
    host: ResourceId,
}

impl ObservationEmitter {
    pub fn new(host: ResourceId) -> Self {
        Self { host }
    }

    pub fn host(&self) -> &ResourceId {
        &self.host
    }

    pub fn memory(&self, now: DateTime<Utc>, mem: &argus_sensors::meminfo::MemInfo) -> Vec<Observation> {
        vec![
            number(&self.host, "memory.total_kib", mem.mem_total as f64, "procfs", "read_meminfo", now),
            number(&self.host, "memory.available_kib", mem.mem_available as f64, "procfs", "read_meminfo", now),
            number(&self.host, "memory.swap_total_kib", mem.swap_total as f64, "procfs", "read_meminfo", now),
        ]
    }

    pub fn load(&self, now: DateTime<Utc>, load: &argus_sensors::loadavg::LoadAverage) -> Vec<Observation> {
        vec![
            number(&self.host, "load.1m", load.load1, "procfs", "read_loadavg", now),
            number(&self.host, "load.5m", load.load5, "procfs", "read_loadavg", now),
            number(&self.host, "load.15m", load.load15, "procfs", "read_loadavg", now),
        ]
    }

    pub fn cpu(&self, now: DateTime<Utc>, cpu: &argus_sensors::stat::CpuTimes) -> Vec<Observation> {
        vec![
            number(&self.host, "cpu.user_ticks", cpu.user as f64, "procfs", "read_stat", now),
            number(&self.host, "cpu.system_ticks", cpu.system as f64, "procfs", "read_stat", now),
            number(&self.host, "cpu.idle_ticks", cpu.idle as f64, "procfs", "read_stat", now),
        ]
    }

    pub fn vm(&self, now: DateTime<Utc>, vm: &argus_sensors::vmstat::VmStat) -> Vec<Observation> {
        vec![
            number(&self.host, "vm.oom_kill", vm.oom_kill as f64, "procfs", "read_vmstat", now),
            number(&self.host, "vm.pgmajfault", vm.pgmajfault as f64, "procfs", "read_vmstat", now),
        ]
    }

    /// Emit one process record as observations. The subject is `process:<pid>`.
    pub fn process(&self, now: DateTime<Utc>, rec: &ProcessRecord) -> Vec<Observation> {
        // pid is a non-empty, whitespace-free string, so this never fails.
        let subject = ResourceId::new("process", &rec.pid.to_string())
            .expect("a numeric pid is a valid resource identifier");
        vec![
            text(&subject, "process.state", &rec.state, "procfs", "read_status", now),
            number(&subject, "process.rss_kib", rec.vm_rss_kib as f64, "procfs", "read_status", now),
            number(&subject, "process.cpu_ticks", (rec.utime + rec.stime) as f64, "procfs", "read_stat", now),
        ]
    }
}

fn number(subject: &ResourceId, attribute: &str, value: f64, src: &str, method: &str, now: DateTime<Utc>) -> Observation {
    emit(subject, attribute, ObservedValue::Number(value), src, method, now)
}

fn text(subject: &ResourceId, attribute: &str, value: &str, src: &str, method: &str, now: DateTime<Utc>) -> Observation {
    emit(subject, attribute, ObservedValue::Text(value.to_string()), src, method, now)
}

fn emit(subject: &ResourceId, attribute: &str, value: ObservedValue, src: &str, method: &str, now: DateTime<Utc>) -> Observation {
    Observation::new(
        Uuid::new_v4(),
        "argusd",
        subject.clone(),
        attribute,
        value,
        1.0,
        Provenance::new(src, method, now),
        now,
    )
    .expect("confidence 1.0 is always within [0,1]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 30, 12, 0, 0).unwrap()
    }

    fn emitter() -> ObservationEmitter {
        ObservationEmitter::new(ResourceId::new("host", "local").unwrap())
    }

    fn value<'a>(obs: &'a [Observation], attribute: &str) -> &'a ObservedValue {
        obs.iter()
            .find(|o| o.attribute() == attribute)
            .unwrap_or_else(|| panic!("no observation for attribute {attribute}"))
            .value()
    }

    #[test]
    fn memory_emits_host_subject() {
        let mem = argus_sensors::meminfo::MemInfo {
            mem_total: 16_384_000,
            mem_available: 10_240_000,
            swap_total: 2_048_000,
            ..Default::default()
        };
        let obs = emitter().memory(now(), &mem);
        assert_eq!(obs.len(), 3);
        for o in &obs {
            assert_eq!(o.subject().kind(), "host");
            assert_eq!(o.source(), "argusd");
        }
        assert_eq!(value(&obs, "memory.total_kib"), &ObservedValue::Number(16_384_000.0));
        assert_eq!(value(&obs, "memory.available_kib"), &ObservedValue::Number(10_240_000.0));
    }

    #[test]
    fn load_emits_three_averages() {
        let load = argus_sensors::loadavg::LoadAverage {
            load1: 0.5,
            load5: 1.5,
            load15: 2.5,
            running_threads: 2,
            total_threads: 300,
            last_pid: 1000,
        };
        let obs = emitter().load(now(), &load);
        assert_eq!(value(&obs, "load.1m"), &ObservedValue::Number(0.5));
        assert_eq!(value(&obs, "load.15m"), &ObservedValue::Number(2.5));
    }

    #[test]
    fn vm_emits_oom_kill() {
        let vm = argus_sensors::vmstat::VmStat {
            oom_kill: 3,
            pgmajfault: 100,
            ..Default::default()
        };
        let obs = emitter().vm(now(), &vm);
        assert_eq!(value(&obs, "vm.oom_kill"), &ObservedValue::Number(3.0));
        assert_eq!(value(&obs, "vm.pgmajfault"), &ObservedValue::Number(100.0));
    }

    #[test]
    fn process_emits_process_subject() {
        let rec = ProcessRecord {
            pid: 1234,
            ppid: 1,
            name: "nginx".to_string(),
            state: "S".to_string(),
            uid: 0,
            threads: 8,
            vm_rss_kib: 12_345,
            cap_eff: 0,
            utime: 100,
            stime: 50,
            start_ticks: 99,
            cmdline: vec!["nginx".to_string()],
            cgroup: Vec::new(),
        };
        let obs = emitter().process(now(), &rec);
        for o in &obs {
            assert_eq!(o.subject().kind(), "process");
            assert_eq!(o.subject().identifier(), "1234");
        }
        assert_eq!(value(&obs, "process.state"), &ObservedValue::Text("S".to_string()));
        assert_eq!(value(&obs, "process.rss_kib"), &ObservedValue::Number(12_345.0));
        assert_eq!(value(&obs, "process.cpu_ticks"), &ObservedValue::Number(150.0));
    }
}
