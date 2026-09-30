//! `/proc/diskstats` reader: block-device I/O statistics.
//!
//! One file lists every block device and partition. Fields are documented in
//! the kernel's `Documentation/admin-guide/iostats.rst`. We surface the
//! completion, sector, and time counters that feed disk-I/O baseline and
//! capacity signals (CAP-1).

use crate::{Sensor, SensorError};

/// I/O statistics for a single block device or partition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskStats {
    pub name: String,
    pub reads_completed: u64,
    pub sectors_read: u64,
    pub time_reading_ms: u64,
    pub writes_completed: u64,
    pub sectors_written: u64,
    pub time_writing_ms: u64,
    pub io_in_progress: u64,
    pub time_io_ms: u64,
    pub weighted_time_io_ms: u64,
}

/// Pure parser over `/proc/diskstats` contents.
pub fn parse(input: &str) -> Result<Vec<DiskStats>, SensorError> {
    const NAME: &str = "host.diskstats";

    let mut disks = Vec::new();
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        // major minor name + 11 stat fields (indices 0..13).
        if fields.len() < 14 {
            continue;
        }
        let u = |i: usize| -> u64 { fields.get(i).and_then(|s| s.parse().ok()).unwrap_or(0) };
        disks.push(DiskStats {
            name: fields[2].to_string(),
            reads_completed: u(3),
            sectors_read: u(5),
            time_reading_ms: u(6),
            writes_completed: u(7),
            sectors_written: u(9),
            time_writing_ms: u(10),
            io_in_progress: u(11),
            time_io_ms: u(12),
            weighted_time_io_ms: u(13),
        });
    }

    if disks.is_empty() {
        return Err(SensorError::Parse {
            name: NAME,
            message: "no disk entries found".to_string(),
        });
    }

    Ok(disks)
}

/// Read `/proc/diskstats`.
pub fn read() -> Result<Vec<DiskStats>, SensorError> {
    let contents = crate::read_file("host.diskstats", "/proc/diskstats")?;
    parse(&contents)
}

/// The `/proc/diskstats` sensor.
#[derive(Debug, Clone, Copy, Default)]
pub struct DiskStatsSensor;

impl Sensor for DiskStatsSensor {
    type Output = Vec<DiskStats>;

    fn name(&self) -> &'static str {
        "host.diskstats"
    }

    fn read(&self) -> Result<Vec<DiskStats>, SensorError> {
        read()
    }

    fn available(&self) -> bool {
        std::path::Path::new("/proc/diskstats").exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "\
   8       0 sda 123456 789 4567890 1234567 987654 321 7654321 2345678 0 345678 4567890 0 0 0 0
   8       1 sda1 123400 0 4567000 1200000 0 0 0 0 0 0 0 0 0 0 0
";

    #[test]
    fn parses_device_and_partition() {
        let disks = parse(FIXTURE).unwrap();
        assert_eq!(disks.len(), 2);

        let sda = &disks[0];
        assert_eq!(sda.name, "sda");
        assert_eq!(sda.reads_completed, 123_456);
        assert_eq!(sda.sectors_read, 4_567_890);
        assert_eq!(sda.time_reading_ms, 1_234_567);
        assert_eq!(sda.writes_completed, 987_654);
        assert_eq!(sda.sectors_written, 7_654_321);
        assert_eq!(sda.time_writing_ms, 2_345_678);
        assert_eq!(sda.io_in_progress, 0);
        assert_eq!(sda.time_io_ms, 345_678);
        assert_eq!(sda.weighted_time_io_ms, 4_567_890);

        assert_eq!(disks[1].name, "sda1");
    }

    #[test]
    fn skips_short_lines() {
        // A truncated line (major/minor/name only) is ignored while a valid line
        // on the same input is still parsed.
        let disks = parse(
            "   8       0 sda\n   8       0 sda 1 2 3 4 5 6 7 8 9 10 11 12 13\n",
        )
        .unwrap();
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].name, "sda");
    }

    #[test]
    fn empty_input_is_a_parse_error() {
        assert!(matches!(
            parse(""),
            Err(SensorError::Parse { name: "host.diskstats", .. })
        ));
    }
}
