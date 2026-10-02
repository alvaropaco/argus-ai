//! `/proc/loadavg` reader: system load averages.
//!
//! Format: `0.00 0.01 0.05 1/234 12345` — one-minute, five-minute and
//! fifteen-minute load averages, `running_threads/total_threads`, and the PID
//! most recently created.

use crate::{Sensor, SensorError};

/// Load averages and runnable/thread counts parsed from `/proc/loadavg`.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq)] // contains f64
pub struct LoadAverage {
    pub load1: f64,
    pub load5: f64,
    pub load15: f64,
    pub running_threads: u32,
    pub total_threads: u32,
    pub last_pid: u32,
}

/// Pure parser over `/proc/loadavg` contents.
pub fn parse(input: &str) -> Result<LoadAverage, SensorError> {
    const NAME: &str = "host.loadavg";

    let tokens: Vec<&str> = input.split_whitespace().collect();
    if tokens.len() < 5 {
        return Err(SensorError::Parse {
            name: NAME,
            message: format!("expected 5 fields, got {}", tokens.len()),
        });
    }

    let (running, total) = tokens[3]
        .split_once('/')
        .ok_or_else(|| SensorError::Parse {
            name: NAME,
            message: format!("invalid threads field `{}`", tokens[3]),
        })?;

    Ok(LoadAverage {
        load1: parse_f64(NAME, tokens[0])?,
        load5: parse_f64(NAME, tokens[1])?,
        load15: parse_f64(NAME, tokens[2])?,
        running_threads: parse_u32(NAME, running)?,
        total_threads: parse_u32(NAME, total)?,
        last_pid: parse_u32(NAME, tokens[4])?,
    })
}

fn parse_f64(name: &'static str, token: &str) -> Result<f64, SensorError> {
    token.parse::<f64>().map_err(|_| SensorError::Parse {
        name,
        message: format!("invalid float `{token}`"),
    })
}

fn parse_u32(name: &'static str, token: &str) -> Result<u32, SensorError> {
    token.parse::<u32>().map_err(|_| SensorError::Parse {
        name,
        message: format!("invalid integer `{token}`"),
    })
}

/// Read `/proc/loadavg`.
pub fn read() -> Result<LoadAverage, SensorError> {
    let contents = crate::read_file("host.loadavg", "/proc/loadavg")?;
    parse(&contents)
}

/// The `/proc/loadavg` sensor.
#[derive(Debug, Clone, Copy, Default)]
pub struct LoadAverageSensor;

impl Sensor for LoadAverageSensor {
    type Output = LoadAverage;

    fn name(&self) -> &'static str {
        "host.loadavg"
    }

    fn read(&self) -> Result<LoadAverage, SensorError> {
        read()
    }

    fn available(&self) -> bool {
        std::path::Path::new("/proc/loadavg").exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_valid_line() {
        let l = parse("0.12 0.31 0.46 2/387 98765").unwrap();
        assert!((l.load1 - 0.12).abs() < 1e-9);
        assert!((l.load5 - 0.31).abs() < 1e-9);
        assert!((l.load15 - 0.46).abs() < 1e-9);
        assert_eq!(l.running_threads, 2);
        assert_eq!(l.total_threads, 387);
        assert_eq!(l.last_pid, 98_765);
    }

    #[test]
    fn rejects_too_few_fields() {
        assert!(matches!(
            parse("0.12 0.31 0.46"),
            Err(SensorError::Parse {
                name: "host.loadavg",
                ..
            })
        ));
    }

    #[test]
    fn rejects_malformed_threads_field() {
        assert!(parse("0.12 0.31 0.46 2-387 98765").is_err());
    }

    #[test]
    fn rejects_non_numeric_load() {
        assert!(parse("abc 0.31 0.46 2/387 98765").is_err());
    }
}
