//! `/proc/<pid>/*` readers: process identity, resource state, and lifecycle.
//!
//! These are the per-pid data sources for the CAP-2 live process inventory.
//! Unlike the host sensors, they read many pids, so they are exposed as free
//! functions rather than a single [`crate::Sensor`] implementation. The
//! inventory coordinator (`argus-observe`) iterates pids and calls them.

use crate::SensorError;

/// Identity and resource state parsed from `/proc/<pid>/status`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProcessStatus {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    /// The process state letter (e.g. `S`, `R`, `Z`).
    pub state: String,
    /// Real UID.
    pub uid: u32,
    pub threads: u32,
    pub vm_rss_kib: u64,
    /// Effective capabilities, as a bitmap.
    pub cap_eff: u64,
}

/// CPU accounting and start time parsed from `/proc/<pid>/stat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProcessStat {
    /// User-mode CPU ticks (field 14).
    pub utime: u64,
    /// Kernel-mode CPU ticks (field 15).
    pub stime: u64,
    /// Start time in ticks since boot (field 22), for PID-reuse detection.
    pub start_ticks: u64,
}

/// Pure parser over `/proc/<pid>/status` contents.
pub fn parse(input: &str) -> Result<ProcessStatus, SensorError> {
    const NAME: &str = "process.status";

    if input.trim().is_empty() {
        return Err(SensorError::Parse {
            name: NAME,
            message: "empty status file".to_string(),
        });
    }

    let mut st = ProcessStatus::default();
    for line in input.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key {
            "Name" => st.name = value.to_string(),
            "Pid" => st.pid = first_u32(value).unwrap_or(0),
            "PPid" => st.ppid = first_u32(value).unwrap_or(0),
            "State" => st.state = value.split_whitespace().next().unwrap_or("").to_string(),
            "Uid" => st.uid = first_u32(value).unwrap_or(0),
            "Threads" => st.threads = first_u32(value).unwrap_or(0),
            "VmRSS" => st.vm_rss_kib = first_u64(value).unwrap_or(0),
            "CapEff" => {
                st.cap_eff = value
                    .split_whitespace()
                    .next()
                    .and_then(|s| u64::from_str_radix(s, 16).ok())
                    .unwrap_or(0);
            }
            _ => {}
        }
    }

    Ok(st)
}

/// Pure parser over `/proc/<pid>/stat` contents.
///
/// The `comm` field (in parentheses) may contain spaces and parentheses, so the
/// split is anchored at the *last* `)`. Fields after it are space-separated and
/// 1-based from the full line: field 14 = utime, 15 = stime, 22 = starttime.
pub fn parse_stat(input: &str) -> Result<ProcessStat, SensorError> {
    const NAME: &str = "process.stat";

    let close = input.rfind(')').ok_or_else(|| SensorError::Parse {
        name: NAME,
        message: "missing comm close parenthesis".to_string(),
    })?;
    let tokens: Vec<&str> = input[close + 1..].split_whitespace().collect();
    // tokens[0]=state(3) tokens[1]=ppid(4) ... tokens[11]=utime(14) tokens[12]=stime(15) ... tokens[19]=starttime(22)
    if tokens.len() < 20 {
        return Err(SensorError::Parse {
            name: NAME,
            message: format!("expected at least 20 stat fields, got {}", tokens.len()),
        });
    }

    Ok(ProcessStat {
        utime: tokens[11].parse().unwrap_or(0),
        stime: tokens[12].parse().unwrap_or(0),
        start_ticks: tokens[19].parse().unwrap_or(0),
    })
}

/// Pure parser over `/proc/<pid>/cmdline` (NUL-separated arguments).
pub fn parse_cmdline(input: &str) -> Vec<String> {
    input
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Pure parser over `/proc/<pid>/cgroup` (one membership line per controller).
pub fn parse_cgroup(input: &str) -> Vec<String> {
    input
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

fn first_u32(value: &str) -> Option<u32> {
    value.split_whitespace().next()?.parse().ok()
}

fn first_u64(value: &str) -> Option<u64> {
    value.split_whitespace().next()?.parse().ok()
}

/// Read `/proc/<pid>/status`.
pub fn read(pid: u32) -> Result<ProcessStatus, SensorError> {
    let contents = crate::read_file("process.status", &format!("/proc/{pid}/status"))?;
    parse(&contents)
}

/// Read `/proc/<pid>/stat`.
pub fn read_stat(pid: u32) -> Result<ProcessStat, SensorError> {
    let contents = crate::read_file("process.stat", &format!("/proc/{pid}/stat"))?;
    parse_stat(&contents)
}

/// Read `/proc/<pid>/cmdline`.
pub fn read_cmdline(pid: u32) -> Result<Vec<String>, SensorError> {
    let contents = crate::read_file("process.cmdline", &format!("/proc/{pid}/cmdline"))?;
    Ok(parse_cmdline(&contents))
}

/// Read `/proc/<pid>/cgroup`.
pub fn read_cgroup(pid: u32) -> Result<Vec<String>, SensorError> {
    let contents = crate::read_file("process.cgroup", &format!("/proc/{pid}/cgroup"))?;
    Ok(parse_cgroup(&contents))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = "\
Name:	nginx
Umask:	0022
State:	S (sleeping)
Pid:	1234
PPid:	1
Uid:	0	0	0	0
Gid:	0	0	0	0
Threads:	8
VmRSS:	   12345 kB
CapInh:	0000000000000000
CapPrm:	0000003fffffffff
CapEff:	0000003fffffffff
CapBnd:	0000003fffffffff
";

    #[test]
    fn parses_identity_and_resources() {
        let p = parse(STATUS).unwrap();
        assert_eq!(p.pid, 1234);
        assert_eq!(p.ppid, 1);
        assert_eq!(p.name, "nginx");
        assert_eq!(p.state, "S");
        assert_eq!(p.uid, 0);
        assert_eq!(p.threads, 8);
        assert_eq!(p.vm_rss_kib, 12_345);
        assert_eq!(p.cap_eff, 0x3f_ffff_ffff);
    }

    #[test]
    fn state_is_only_the_letter() {
        let p = parse(STATUS).unwrap();
        assert_eq!(p.state, "S");
    }

    #[test]
    fn empty_input_is_a_parse_error() {
        assert!(matches!(
            parse(""),
            Err(SensorError::Parse {
                name: "process.status",
                ..
            })
        ));
    }

    const STAT: &str = "\
1234 (nginx) S 1 1234 1234 0 -1 4194560 100 0 0 0 250 150 0 0 20 0 8 0 99999999 123456789 5678 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0";

    #[test]
    fn parses_cpu_and_start_ticks() {
        let s = parse_stat(STAT).unwrap();
        assert_eq!(s.utime, 250);
        assert_eq!(s.stime, 150);
        assert_eq!(s.start_ticks, 99_999_999);
    }

    #[test]
    fn stat_comm_may_contain_spaces_and_parens() {
        let s = parse_stat("42 (a (b) c) S 1 0 0 0 -1 0 0 0 0 0 11 12 0 0 20 0 1 0 777 0 0 0 0 0")
            .unwrap();
        assert_eq!(s.utime, 11);
        assert_eq!(s.stime, 12);
        assert_eq!(s.start_ticks, 777);
    }

    #[test]
    fn stat_without_close_paren_is_an_error() {
        assert!(parse_stat("1234 no-paren 1 2 3").is_err());
    }

    #[test]
    fn parses_null_separated_cmdline() {
        assert_eq!(
            parse_cmdline("nginx\0-master\0process\0"),
            vec!["nginx", "-master", "process"]
        );
        assert_eq!(parse_cmdline(""), Vec::<String>::new());
    }

    #[test]
    fn parses_cgroup_lines() {
        let c =
            parse_cgroup("0::/user.slice/user-1000.slice/session.scope\n12:cpuset:/docker/abc\n");
        assert_eq!(
            c,
            vec![
                "0::/user.slice/user-1000.slice/session.scope",
                "12:cpuset:/docker/abc"
            ]
        );
    }
}
