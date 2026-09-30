//! `/proc/pressure/*` readers: Pressure Stall Information (PSI).
//!
//! Each file (`cpu`, `memory`, `io`) reports `some` (some tasks stalled) and,
//! for cpu/io, `full` (all tasks stalled). Values are averages over 10s/60s/300s
//! windows plus a cumulative `total` stall time (µs). PSI is a first-class
//! contention signal, distinct from raw utilization (Principle 5, domain-model §4.4).

use crate::{Sensor, SensorError};

/// A single PSI line (`some` or `full`).
#[derive(Debug, Clone, Copy, Default)]
pub struct PressureLine {
    pub avg10: f64,
    pub avg60: f64,
    pub avg300: f64,
    pub total: u64,
}

/// The `some` and `full` lines of one pressure file. `full` is `None` for
/// memory, which only reports `some`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pressure {
    pub some: Option<PressureLine>,
    pub full: Option<PressureLine>,
}

/// Pure parser over one PSI file's contents.
pub fn parse(input: &str) -> Result<Pressure, SensorError> {
    const NAME: &str = "host.pressure";

    let mut pressure = Pressure::default();
    for line in input.lines() {
        let Some((tag, pl)) = parse_line(line) else {
            continue;
        };
        match tag {
            "some" => pressure.some = Some(pl),
            "full" => pressure.full = Some(pl),
            _ => {}
        }
    }

    if pressure.some.is_none() && pressure.full.is_none() {
        return Err(SensorError::Parse {
            name: NAME,
            message: "no `some` or `full` line found".to_string(),
        });
    }

    Ok(pressure)
}

fn parse_line(line: &str) -> Option<(&str, PressureLine)> {
    let mut it = line.split_whitespace();
    let tag = it.next()?;
    let mut pl = PressureLine::default();
    for kv in it {
        let (key, value) = kv.split_once('=')?;
        match key {
            "avg10" => pl.avg10 = value.parse().ok()?,
            "avg60" => pl.avg60 = value.parse().ok()?,
            "avg300" => pl.avg300 = value.parse().ok()?,
            "total" => pl.total = value.parse().ok()?,
            _ => {}
        }
    }
    Some((tag, pl))
}

fn read_pressure(name: &'static str, path: &str) -> Result<Pressure, SensorError> {
    let contents = crate::read_file(name, path)?;
    parse(&contents)
}

/// Read `/proc/pressure/cpu`.
pub fn read_cpu() -> Result<Pressure, SensorError> {
    read_pressure("host.pressure.cpu", "/proc/pressure/cpu")
}

/// Read `/proc/pressure/memory`.
pub fn read_memory() -> Result<Pressure, SensorError> {
    read_pressure("host.pressure.memory", "/proc/pressure/memory")
}

/// Read `/proc/pressure/io`.
pub fn read_io() -> Result<Pressure, SensorError> {
    read_pressure("host.pressure.io", "/proc/pressure/io")
}

macro_rules! pressure_sensor {
    ($name:ident, $label:literal, $path:literal, $read:ident) => {
        #[derive(Debug, Clone, Copy, Default)]
        pub struct $name;

        impl Sensor for $name {
            type Output = Pressure;

            fn name(&self) -> &'static str {
                $label
            }

            fn read(&self) -> Result<Pressure, SensorError> {
                $read()
            }

            fn available(&self) -> bool {
                std::path::Path::new($path).exists()
            }
        }
    };
}

pressure_sensor!(CpuPressureSensor, "host.pressure.cpu", "/proc/pressure/cpu", read_cpu);
pressure_sensor!(MemoryPressureSensor, "host.pressure.memory", "/proc/pressure/memory", read_memory);
pressure_sensor!(IoPressureSensor, "host.pressure.io", "/proc/pressure/io", read_io);

#[cfg(test)]
mod tests {
    use super::*;

    const CPU_FIXTURE: &str = "\
some avg10=0.12 avg60=0.31 avg300=0.46 total=123456789
full avg10=0.00 avg60=0.01 avg300=0.02 total=98765
";

    const MEMORY_FIXTURE: &str = "\
some avg10=0.00 avg60=0.00 avg300=0.00 total=0
";

    #[test]
    fn parses_some_and_full_lines() {
        let p = parse(CPU_FIXTURE).unwrap();
        let some = p.some.unwrap();
        assert!((some.avg10 - 0.12).abs() < 1e-9);
        assert!((some.avg60 - 0.31).abs() < 1e-9);
        assert!((some.avg300 - 0.46).abs() < 1e-9);
        assert_eq!(some.total, 123_456_789);
        let full = p.full.unwrap();
        assert!((full.avg10 - 0.0).abs() < 1e-9);
        assert_eq!(full.total, 98_765);
    }

    #[test]
    fn memory_only_reports_some() {
        let p = parse(MEMORY_FIXTURE).unwrap();
        assert!(p.some.is_some());
        assert!(p.full.is_none());
    }

    #[test]
    fn empty_input_is_a_parse_error() {
        assert!(matches!(
            parse(""),
            Err(SensorError::Parse { name: "host.pressure", .. })
        ));
    }
}
