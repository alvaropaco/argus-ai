//! statvfs disk-usage reader (spec 011 FR-003): one mount, percent used.
//!
//! Kernel-native: a statvfs(2) read through `rustix` (the audited syscall
//! boundary — the workspace forbids `unsafe`, so the raw call lives in the
//! dependency, never here). No command execution, no new dependency tree.

use crate::SensorError;

/// The default mount the situation collector watches: the root filesystem.
pub const DEFAULT_MOUNT: &str = "/";

/// Disk-usage figures for one mount (spec 011 FR-003).
///
/// The percent follows the `df` convention: `(blocks − bavail) / blocks` —
/// usage against the space an unprivileged process can actually still fill.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DiskUsage {
    /// The mount the reading describes (e.g. `/`).
    pub mount: String,
    /// Percent of the filesystem in use, `0.0` when the size is unknown so a
    /// degenerate statvfs never reads as full saturation (the meminfo
    /// convention).
    pub used_percent: f64,
    /// Total filesystem size in bytes.
    pub total: u64,
    /// Free bytes available to unprivileged users (`f_bavail × f_frsize`).
    pub free: u64,
}

impl DiskUsage {
    /// Pure constructor over raw statvfs figures, so the percent math is
    /// testable without a kernel (Principle 13).
    pub fn from_blocks(mount: impl Into<String>, frsize: u64, blocks: u64, bavail: u64) -> Self {
        let used = blocks.saturating_sub(bavail);
        let used_percent = if blocks == 0 {
            0.0
        } else {
            used as f64 / blocks as f64 * 100.0
        };
        Self {
            mount: mount.into(),
            used_percent,
            total: blocks.saturating_mul(frsize),
            free: bavail.saturating_mul(frsize),
        }
    }
}

/// Reads disk usage for `mount` via statvfs. A failed read is an error: the
/// caller contributes no situation for it (fail-closed, spec 011 NFR) —
/// never a fabricated one.
pub fn usage(mount: &str) -> Result<DiskUsage, SensorError> {
    const NAME: &str = "host.disk";
    let stat = rustix::fs::statvfs(mount).map_err(|source| SensorError::Read {
        name: NAME,
        source: source.into(),
    })?;
    Ok(DiskUsage::from_blocks(
        mount,
        stat.f_frsize,
        stat.f_blocks,
        stat.f_bavail,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_math_follows_the_df_convention() {
        // (16_384_000 − 10_240_000) / 16_384_000 = 37.5%, the meminfo fixture's
        // mirror so the two readers compute the same way.
        let usage = DiskUsage::from_blocks("/", 4096, 16_384_000, 10_240_000);
        assert_eq!(usage.mount, "/");
        assert!((usage.used_percent - 37.5).abs() < 1e-9);
        assert_eq!(usage.total, 16_384_000u64 * 4096);
        assert_eq!(usage.free, 10_240_000u64 * 4096);
    }

    #[test]
    fn full_and_empty_filesystems_read_as_their_bounds() {
        let full = DiskUsage::from_blocks("/", 512, 1_000, 0);
        assert!((full.used_percent - 100.0).abs() < 1e-9);
        let empty = DiskUsage::from_blocks("/", 512, 1_000, 1_000);
        assert_eq!(empty.used_percent, 0.0);
        assert_eq!(empty.free, 1_000u64 * 512);
    }

    #[test]
    fn zero_blocks_yield_zero_percent_never_saturation() {
        let usage = DiskUsage::from_blocks("/", 4096, 0, 0);
        assert_eq!(usage.used_percent, 0.0);
        assert_eq!(usage.total, 0);
        assert_eq!(usage.free, 0);
    }

    #[test]
    fn available_larger_than_blocks_clamps_to_zero_used() {
        // A weird statvfs must not read as negative usage.
        let usage = DiskUsage::from_blocks("/", 4096, 100, 200);
        assert_eq!(usage.used_percent, 0.0);
    }

    #[test]
    fn a_live_statvfs_read_on_the_root_mount_is_sane() {
        // statvfs on `/` is fine in CI (spec 011): the real kernel read.
        let usage = usage(DEFAULT_MOUNT).expect("the root mount is readable");
        assert_eq!(usage.mount, "/");
        assert!(usage.total > 0, "a real filesystem has a size");
        assert!(usage.free <= usage.total);
        assert!(
            (0.0..=100.0).contains(&usage.used_percent),
            "percent stays in range: {}",
            usage.used_percent
        );
    }
}
