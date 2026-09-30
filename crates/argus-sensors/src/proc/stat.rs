//! `/proc/stat` reader: aggregate CPU time accounting.
//!
//! Kernel documentation: <https://docs.kernel.org/filesystems/proc.html>.
//! The first `cpu` line aggregates all CPUs; values are in USER_HZ ticks.

use crate::{Sensor, SensorError};

/// CPU time accounting for the aggregate `cpu` line of `/proc/stat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuTimes {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuTimes {
    /// Time spent idle, including iowait (the Linux convention used by `top`).
    pub fn idle_ticks(&self) -> u64 {
        self.idle + self.iowait
    }

    /// Total accounted time across all fields.
    pub fn total(&self) -> u64 {
        self.user + self.nice + self.system + self.idle + self.iowait + self.irq + self.softirq
            + self.steal
    }

    /// CPU utilization percentage over the interval `[prev, self]`.
    /// Returns `0.0` when no ticks elapsed (avoids a divide-by-zero).
    pub fn utilization_since(&self, prev: &CpuTimes) -> f64 {
        let total_delta = self.total().saturating_sub(prev.total());
        if total_delta == 0 {
            return 0.0;
        }
        let idle_delta = self.idle_ticks().saturating_sub(prev.idle_ticks());
        (total_delta - idle_delta) as f64 / total_delta as f64 * 100.0
    }
}

/// Pure parser over `/proc/stat` contents (returns the aggregate `cpu` line).
pub fn parse(input: &str) -> Result<CpuTimes, SensorError> {
    const NAME: &str = "host.cpu";

    let line = input
        .lines()
        .find(|l| l.starts_with("cpu "))
        .ok_or_else(|| SensorError::Parse {
            name: NAME,
            message: "no aggregate `cpu` line found".to_string(),
        })?;

    let fields: Vec<&str> = line.split_whitespace().collect();
    // fields[0] == "cpu"; fields[1..] == user nice system idle [iowait irq softirq steal].
    if fields.len() < 5 {
        return Err(SensorError::Parse {
            name: NAME,
            message: format!("expected at least 4 time fields, got {}", fields.len() - 1),
        });
    }

    Ok(CpuTimes {
        user: required_tick(NAME, &fields, 1)?,
        nice: required_tick(NAME, &fields, 2)?,
        system: required_tick(NAME, &fields, 3)?,
        idle: required_tick(NAME, &fields, 4)?,
        iowait: optional_tick(&fields, 5),
        irq: optional_tick(&fields, 6),
        softirq: optional_tick(&fields, 7),
        steal: optional_tick(&fields, 8),
    })
}

fn required_tick(name: &'static str, fields: &[&str], i: usize) -> Result<u64, SensorError> {
    fields
        .get(i)
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| SensorError::Parse {
            name,
            message: format!("invalid cpu tick at field {i}"),
        })
}

fn optional_tick(fields: &[&str], i: usize) -> u64 {
    fields.get(i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0)
}

/// Read `/proc/stat`.
pub fn read() -> Result<CpuTimes, SensorError> {
    let contents = crate::read_file("host.cpu", "/proc/stat")?;
    parse(&contents)
}

/// The `/proc/stat` aggregate-CPU sensor.
#[derive(Debug, Clone, Copy, Default)]
pub struct CpuStatSensor;

impl Sensor for CpuStatSensor {
    type Output = CpuTimes;

    fn name(&self) -> &'static str {
        "host.cpu"
    }

    fn read(&self) -> Result<CpuTimes, SensorError> {
        read()
    }

    fn available(&self) -> bool {
        std::path::Path::new("/proc/stat").exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "\
cpu  2700 120 950 455000 220 0 120 0 0 0
cpu0 300 10 100 50000 20 0 10 0 0 0
intr 123456 0 1 2 3
ctxt 987654
btime 1720000000
processes 12345
procs_running 2
procs_blocked 0
";

    #[test]
    fn parses_aggregate_cpu_line() {
        let c = parse(FIXTURE).unwrap();
        assert_eq!(c.user, 2700);
        assert_eq!(c.nice, 120);
        assert_eq!(c.system, 950);
        assert_eq!(c.idle, 455_000);
        assert_eq!(c.iowait, 220);
        assert_eq!(c.irq, 0);
        assert_eq!(c.softirq, 120);
        assert_eq!(c.steal, 0);
    }

    #[test]
    fn rejects_input_without_aggregate_cpu_line() {
        assert!(matches!(
            parse("intr 1\nctxt 2\n"),
            Err(SensorError::Parse { name: "host.cpu", .. })
        ));
    }

    #[test]
    fn utilization_is_busy_over_total_delta() {
        let prev = CpuTimes {
            user: 100,
            system: 100,
            idle: 800,
            ..Default::default()
        };
        let curr = CpuTimes {
            user: 150,
            system: 150,
            idle: 900,
            ..Default::default()
        };
        // total delta = 200, idle delta = 100 -> 50%.
        assert!((curr.utilization_since(&prev) - 50.0).abs() < 1e-9);
    }

    #[test]
    fn utilization_is_zero_when_no_ticks_elapsed() {
        let c = CpuTimes {
            user: 100,
            idle: 800,
            ..Default::default()
        };
        assert_eq!(c.utilization_since(&c), 0.0);
    }
}
