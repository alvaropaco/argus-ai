//! `/proc/net/dev` reader: per-interface RX/TX byte, packet, error, and drop
//! counters.
//!
//! This is the kernel-native traffic + errors/drops source (CAP-1). The richer
//! address/route/neighbor model is Netlink (a later Linux-gated adapter).

use crate::{Sensor, SensorError};

/// Per-interface counters from `/proc/net/dev`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetDevLine {
    pub name: String,
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub rx_errors: u64,
    pub rx_dropped: u64,
    pub tx_bytes: u64,
    pub tx_packets: u64,
    pub tx_errors: u64,
    pub tx_dropped: u64,
}

/// Pure parser over `/proc/net/dev` contents. Sorted by interface name.
pub fn parse(input: &str) -> Result<Vec<NetDevLine>, SensorError> {
    const NAME: &str = "host.network";

    let mut out = Vec::new();
    for line in input.lines() {
        let line = line.trim();
        // Interface lines are `<iface>: <16 counters>`; header lines have no
        // colon and are skipped.
        let Some((iface, rest)) = line.split_once(':') else {
            continue;
        };
        let iface = iface.trim();
        if iface.is_empty() {
            continue;
        }
        let fields: Vec<&str> = rest.split_whitespace().collect();
        if fields.len() < 16 {
            continue;
        }
        let u = |i: usize| {
            fields
                .get(i)
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
        };
        out.push(NetDevLine {
            name: iface.to_string(),
            rx_bytes: u(0),
            rx_packets: u(1),
            rx_errors: u(2),
            rx_dropped: u(3),
            tx_bytes: u(8),
            tx_packets: u(9),
            tx_errors: u(10),
            tx_dropped: u(11),
        });
    }

    if out.is_empty() {
        return Err(SensorError::Parse {
            name: NAME,
            message: "no interface lines found".to_string(),
        });
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Read `/proc/net/dev`.
pub fn read() -> Result<Vec<NetDevLine>, SensorError> {
    let contents = crate::read_file("host.network", "/proc/net/dev")?;
    parse(&contents)
}

/// The `/proc/net/dev` sensor.
#[derive(Debug, Clone, Copy, Default)]
pub struct NetDevSensor;

impl Sensor for NetDevSensor {
    type Output = Vec<NetDevLine>;

    fn name(&self) -> &'static str {
        "host.network"
    }

    fn read(&self) -> Result<Vec<NetDevLine>, SensorError> {
        read()
    }

    fn available(&self) -> bool {
        std::path::Path::new("/proc/net/dev").exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 1000 10 0 0 0 0 0 0 1000 10 0 0 0 0 0 0
  eth0: 2000 20 5 3 0 0 0 0 3000 30 0 1 0 0 0 0
";

    #[test]
    fn parses_interfaces_sorted() {
        let lines = parse(FIXTURE).unwrap();
        assert_eq!(lines.len(), 2);

        assert_eq!(lines[0].name, "eth0");
        assert_eq!(lines[0].rx_bytes, 2000);
        assert_eq!(lines[0].rx_packets, 20);
        assert_eq!(lines[0].rx_errors, 5);
        assert_eq!(lines[0].rx_dropped, 3);
        assert_eq!(lines[0].tx_bytes, 3000);
        assert_eq!(lines[0].tx_packets, 30);
        assert_eq!(lines[0].tx_errors, 0);
        assert_eq!(lines[0].tx_dropped, 1);

        assert_eq!(lines[1].name, "lo");
        assert_eq!(lines[1].rx_bytes, 1000);
        assert_eq!(lines[1].tx_bytes, 1000);
    }

    #[test]
    fn skips_header_lines_and_short_lines() {
        let lines = parse("    lo: 1 2 3 4 5 6 7 8 1 2 3 4 5 6 7 8\n    short: 1 2 3\n").unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].name, "lo");
    }

    #[test]
    fn empty_input_is_a_parse_error() {
        assert!(matches!(
            parse(""),
            Err(SensorError::Parse {
                name: "host.network",
                ..
            })
        ));
    }
}
