//! `/proc/vmstat` reader: virtual-memory statistics.
//!
//! Format is `key value` (no colon). We surface the counters that feed memory
//! pressure and OOM signals (CAP-1/CAP-7).

use std::collections::HashMap;

use crate::{Sensor, SensorError};

/// Selected virtual-memory counters from `/proc/vmstat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VmStat {
    /// Number of times the OOM killer terminated a task (cumulative).
    pub oom_kill: u64,
    /// Major page faults.
    pub pgmajfault: u64,
    /// Pages swapped in / out.
    pub pswpin: u64,
    pub pswpout: u64,
    /// Pages paged in / out from storage.
    pub pgpgin: u64,
    pub pgpgout: u64,
}

/// Pure parser over `/proc/vmstat` contents.
pub fn parse(input: &str) -> Result<VmStat, SensorError> {
    const NAME: &str = "host.vmstat";

    let mut map: HashMap<&str, u64> = HashMap::new();
    for line in input.lines() {
        let mut it = line.split_whitespace();
        let (Some(key), Some(value)) = (it.next(), it.next()) else {
            continue;
        };
        if let Ok(v) = value.parse::<u64>() {
            map.insert(key, v);
        }
    }

    if map.is_empty() {
        return Err(SensorError::Parse {
            name: NAME,
            message: "no key/value pairs found".to_string(),
        });
    }

    Ok(VmStat {
        oom_kill: map.get("oom_kill").copied().unwrap_or(0),
        pgmajfault: map.get("pgmajfault").copied().unwrap_or(0),
        pswpin: map.get("pswpin").copied().unwrap_or(0),
        pswpout: map.get("pswpout").copied().unwrap_or(0),
        pgpgin: map.get("pgpgin").copied().unwrap_or(0),
        pgpgout: map.get("pgpgout").copied().unwrap_or(0),
    })
}

/// Read `/proc/vmstat`.
pub fn read() -> Result<VmStat, SensorError> {
    let contents = crate::read_file("host.vmstat", "/proc/vmstat")?;
    parse(&contents)
}

/// The `/proc/vmstat` sensor.
#[derive(Debug, Clone, Copy, Default)]
pub struct VmStatSensor;

impl Sensor for VmStatSensor {
    type Output = VmStat;

    fn name(&self) -> &'static str {
        "host.vmstat"
    }

    fn read(&self) -> Result<VmStat, SensorError> {
        read()
    }

    fn available(&self) -> bool {
        std::path::Path::new("/proc/vmstat").exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "\
oom_kill 3
pgmajfault 15234
pswpin 0
pswpout 12
pgpgin 4096
pgpgout 8192
nr_free_pages 50000
";

    #[test]
    fn parses_known_counters() {
        let v = parse(FIXTURE).unwrap();
        assert_eq!(v.oom_kill, 3);
        assert_eq!(v.pgmajfault, 15_234);
        assert_eq!(v.pswpin, 0);
        assert_eq!(v.pswpout, 12);
        assert_eq!(v.pgpgin, 4096);
        assert_eq!(v.pgpgout, 8192);
    }

    #[test]
    fn missing_counters_default_to_zero() {
        let v = parse("oom_kill 1\n").unwrap();
        assert_eq!(v.oom_kill, 1);
        assert_eq!(v.pgmajfault, 0);
    }

    #[test]
    fn empty_input_is_a_parse_error() {
        assert!(matches!(
            parse(""),
            Err(SensorError::Parse { name: "host.vmstat", .. })
        ));
    }
}
