//! `/proc/meminfo` reader: host memory and swap totals.
//!
//! Kernel documentation: <https://docs.kernel.org/filesystems/proc.html>.
//! Format: `Key: <value> <unit>` where values are in KiB.

use std::collections::HashMap;

use crate::{Sensor, SensorError};

/// Memory and swap figures parsed from `/proc/meminfo` (all values in KiB).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MemInfo {
    pub mem_total: u64,
    pub mem_free: u64,
    pub mem_available: u64,
    pub buffers: u64,
    pub cached: u64,
    pub swap_total: u64,
    pub swap_free: u64,
}

impl MemInfo {
    /// Percentage of memory in use, computed as
    /// `(mem_total - mem_available) / mem_total`. Returns `0.0` when total is
    /// unknown so a missing value never reads as full saturation.
    pub fn used_percent(&self) -> f64 {
        if self.mem_total == 0 {
            0.0
        } else {
            (self.mem_total - self.mem_available) as f64 / self.mem_total as f64 * 100.0
        }
    }
}

/// Pure parser over `/proc/meminfo` contents.
pub fn parse(input: &str) -> Result<MemInfo, SensorError> {
    const NAME: &str = "host.meminfo";

    let mut map: HashMap<&str, u64> = HashMap::new();
    for line in input.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        // Value is the first whitespace-separated token (KiB); the unit is ignored.
        let Some(value) = rest.split_whitespace().next() else {
            continue;
        };
        let Ok(kib) = value.parse::<u64>() else {
            continue;
        };
        map.insert(key.trim(), kib);
    }

    if map.is_empty() {
        return Err(SensorError::Parse {
            name: NAME,
            message: "no key/value lines found".to_string(),
        });
    }

    Ok(MemInfo {
        mem_total: map.get("MemTotal").copied().unwrap_or(0),
        mem_free: map.get("MemFree").copied().unwrap_or(0),
        mem_available: map.get("MemAvailable").copied().unwrap_or(0),
        buffers: map.get("Buffers").copied().unwrap_or(0),
        cached: map.get("Cached").copied().unwrap_or(0),
        swap_total: map.get("SwapTotal").copied().unwrap_or(0),
        swap_free: map.get("SwapFree").copied().unwrap_or(0),
    })
}

/// Read `/proc/meminfo`.
pub fn read() -> Result<MemInfo, SensorError> {
    let contents = crate::read_file("host.meminfo", "/proc/meminfo")?;
    parse(&contents)
}

/// The `/proc/meminfo` sensor.
#[derive(Debug, Clone, Copy, Default)]
pub struct MemInfoSensor;

impl Sensor for MemInfoSensor {
    type Output = MemInfo;

    fn name(&self) -> &'static str {
        "host.meminfo"
    }

    fn read(&self) -> Result<MemInfo, SensorError> {
        read()
    }

    fn available(&self) -> bool {
        std::path::Path::new("/proc/meminfo").exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "\
MemTotal:       16384000 kB
MemFree:         8192000 kB
MemAvailable:   10240000 kB
Buffers:          204800 kB
Cached:          3072000 kB
SwapCached:            0 kB
Active:          4096000 kB
SwapTotal:       2048000 kB
SwapFree:        2048000 kB
";

    #[test]
    fn parses_known_keys() {
        let m = parse(FIXTURE).unwrap();
        assert_eq!(m.mem_total, 16_384_000);
        assert_eq!(m.mem_free, 8_192_000);
        assert_eq!(m.mem_available, 10_240_000);
        assert_eq!(m.buffers, 204_800);
        assert_eq!(m.cached, 3_072_000);
        assert_eq!(m.swap_total, 2_048_000);
        assert_eq!(m.swap_free, 2_048_000);
    }

    #[test]
    fn ignores_unknown_keys_and_units() {
        let m = parse(FIXTURE).unwrap();
        // SwapCached and Active are not part of MemInfo and must be ignored.
        assert_eq!(m.mem_total, 16_384_000);
    }

    #[test]
    fn used_percent_is_derived_from_available() {
        let m = parse(FIXTURE).unwrap();
        // (16_384_000 - 10_240_000) / 16_384_000 * 100 = 37.5
        assert!((m.used_percent() - 37.5).abs() < 1e-9);
    }

    #[test]
    fn zero_total_yields_zero_percent() {
        let m = MemInfo::default();
        assert_eq!(m.used_percent(), 0.0);
    }

    #[test]
    fn empty_input_is_a_parse_error() {
        assert!(matches!(
            parse(""),
            Err(SensorError::Parse { name: "host.meminfo", .. })
        ));
    }
}
