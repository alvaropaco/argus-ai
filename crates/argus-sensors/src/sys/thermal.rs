//! `/sys/class/thermal` reader: temperature zones.
//!
//! Each `thermal_zoneN/` directory carries a `type` (e.g. `x86_pkg_temp`) and a
//! `temp` file in millidegrees Celsius. Temperatures are "where available"
//! (CAP-1): absent zones degrade to an empty list, never an error.

use std::path::Path;

use crate::SensorError;

/// One temperature zone.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::derive_partial_eq_without_eq)] // f64
pub struct ThermalZone {
    pub name: String,
    pub zone_type: String,
    pub temp_celsius: f64,
}

/// Parse a `temp` file (millidegrees Celsius) into degrees Celsius.
pub fn parse_temp(input: &str) -> Option<f64> {
    input
        .trim()
        .parse::<i64>()
        .ok()
        .map(|millideg| millideg as f64 / 1000.0)
}

/// List thermal zones under `base` (production base: `/sys/class/thermal`).
/// Sorted by name for determinism.
pub fn read_zones(base: &Path) -> Result<Vec<ThermalZone>, SensorError> {
    let entries = std::fs::read_dir(base).map_err(|source| SensorError::Read {
        name: "host.thermal",
        source,
    })?;

    let mut zones = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().into_string().ok() else {
            continue;
        };
        if !name.starts_with("thermal_zone") {
            continue;
        }
        let dir = entry.path();
        let zone_type = std::fs::read_to_string(dir.join("type"))
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let temp_celsius = std::fs::read_to_string(dir.join("temp"))
            .ok()
            .and_then(|s| parse_temp(&s))
            .unwrap_or(0.0);
        zones.push(ThermalZone {
            name,
            zone_type,
            temp_celsius,
        });
    }
    zones.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(zones)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parses_millidegrees_to_celsius() {
        assert!((parse_temp("42500").unwrap() - 42.5).abs() < 1e-9);
        assert!((parse_temp("-5000").unwrap() + 5.0).abs() < 1e-9);
        assert_eq!(parse_temp("not-a-number"), None);
    }

    fn fixture_root() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("argus-thermal-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(root.join("thermal_zone0")).unwrap();
        std::fs::create_dir_all(root.join("thermal_zone1")).unwrap();
        std::fs::create_dir_all(root.join("cooling_device0")).unwrap();
        std::fs::write(root.join("thermal_zone0/type"), "x86_pkg_temp").unwrap();
        std::fs::write(root.join("thermal_zone0/temp"), "42500").unwrap();
        std::fs::write(root.join("thermal_zone1/type"), "acpitz").unwrap();
        std::fs::write(root.join("thermal_zone1/temp"), "30500").unwrap();
        root
    }

    #[test]
    fn lists_only_thermal_zones_sorted() {
        let root = fixture_root();
        let zones = read_zones(&root).unwrap();
        assert_eq!(zones.len(), 2);
        assert_eq!(zones[0].name, "thermal_zone0");
        assert_eq!(zones[0].zone_type, "x86_pkg_temp");
        assert!((zones[0].temp_celsius - 42.5).abs() < 1e-9);
        assert_eq!(zones[1].name, "thermal_zone1");
        assert!((zones[1].temp_celsius - 30.5).abs() < 1e-9);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_base_is_an_error() {
        let root = PathBuf::from("/nonexistent/thermal/path");
        assert!(read_zones(&root).is_err());
    }
}
