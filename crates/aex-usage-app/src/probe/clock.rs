//! The four OS-facing seams the probe needs, and their single Linux
//! implementation each.
//!
//! Every capability that touches the host is a trait, so the whole probe is
//! deterministically testable off Linux and the syscall surface stays exactly
//! four seams. A missing cgroup file is a hard startup error and never a zero
//! reading: silently reading zero physical CPU would make the reconciler's
//! `charged <= physical` cap vacuously true and drop every charge to nothing.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use time::OffsetDateTime;

use super::ProbeError;

/// A monotone reading of the calling thread's own consumed CPU time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CpuInstant(u64);

impl CpuInstant {
    /// Builds a reading from whole microseconds.
    #[must_use]
    pub const fn from_micros(micros: u64) -> Self {
        Self(micros)
    }

    /// The reading in whole microseconds.
    #[must_use]
    pub const fn as_micros(self) -> u64 {
        self.0
    }

    /// Microseconds elapsed since `earlier`, saturating at zero.
    ///
    /// A thread clock is monotone, but a task can migrate between threads, so a
    /// backwards delta is treated as zero elapsed rather than as an underflow.
    #[must_use]
    pub const fn saturating_since(self, earlier: Self) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

/// Whole CPU microseconds attributed by one measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct CpuMicros(u64);

impl CpuMicros {
    /// Builds a microsecond count.
    #[must_use]
    pub const fn new(micros: u64) -> Self {
        Self(micros)
    }

    /// The count in whole microseconds.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Reads the calling thread's consumed CPU time.
pub trait ThreadCpuClock: std::fmt::Debug + Send + Sync + 'static {
    /// The calling thread's consumed CPU time right now.
    fn thread_cpu_now(&self) -> CpuInstant;
}

/// Reads the whole task's physically consumed CPU time.
pub trait PhysicalCpuSource: std::fmt::Debug + Send + Sync + 'static {
    /// Cumulative CPU microseconds the task has consumed.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::PhysicalSource`] when the reading is unavailable or
    /// unparseable. It is never reported as zero.
    fn task_cpu_usec(&self) -> Result<u64, ProbeError>;
}

/// Reads the whole task's current resident memory.
pub trait PhysicalMemorySource: std::fmt::Debug + Send + Sync + 'static {
    /// The task's current memory usage in bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::PhysicalSource`] when the reading is unavailable or
    /// unparseable.
    fn task_memory_current(&self) -> Result<u64, ProbeError>;
}

/// Reads wall-clock time and a monotone instant.
pub trait WallClock: std::fmt::Debug + Send + Sync + 'static {
    /// The current wall-clock instant, for service-time stamping.
    fn now(&self) -> OffsetDateTime;
    /// A monotone instant, for measuring held durations.
    fn instant(&self) -> Instant;
}

/// `clock_gettime(CLOCK_THREAD_CPUTIME_ID)` through `rustix`'s safe wrapper.
#[derive(Debug, Clone, Copy, Default)]
pub struct RustixThreadCpuClock;

impl ThreadCpuClock for RustixThreadCpuClock {
    #[cfg(target_os = "linux")]
    fn thread_cpu_now(&self) -> CpuInstant {
        let spec = rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime);
        let secs = u64::try_from(spec.tv_sec).unwrap_or(0);
        let nanos = u64::try_from(spec.tv_nsec).unwrap_or(0);
        CpuInstant::from_micros(secs.saturating_mul(1_000_000) + nanos / 1_000)
    }

    #[cfg(not(target_os = "linux"))]
    fn thread_cpu_now(&self) -> CpuInstant {
        // The probe only ever runs on Linux. Off-Linux this exists so the crate
        // builds and every test uses an injected fake clock instead; a real
        // measurement here would be a fabricated one.
        CpuInstant::from_micros(0)
    }
}

/// The system wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemWallClock;

impl WallClock for SystemWallClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    fn instant(&self) -> Instant {
        Instant::now()
    }
}

/// The default cgroup v2 CPU accounting file.
pub const CGROUP_V2_CPU_STAT: &str = "/sys/fs/cgroup/cpu.stat";
/// The default cgroup v2 current-memory file.
pub const CGROUP_V2_MEMORY_CURRENT: &str = "/sys/fs/cgroup/memory.current";

/// `cpu.stat` -> `usage_usec`.
#[derive(Debug, Clone)]
pub struct CgroupV2Cpu {
    path: PathBuf,
}

impl CgroupV2Cpu {
    /// Binds the reader to a path and proves the file is readable now.
    ///
    /// The path is configuration, and a missing file is a hard startup error.
    /// Deferring the failure to the first tick would let a task run for minutes
    /// producing no compute facts at all.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::PhysicalSource`] when the file cannot be read or
    /// does not carry a `usage_usec` line.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ProbeError> {
        let source = Self {
            path: path.as_ref().to_path_buf(),
        };
        source.task_cpu_usec()?;
        Ok(source)
    }
}

impl PhysicalCpuSource for CgroupV2Cpu {
    fn task_cpu_usec(&self) -> Result<u64, ProbeError> {
        let text = fs::read_to_string(&self.path).map_err(|error| ProbeError::PhysicalSource {
            source_name: "cgroup cpu.stat",
            path: self.path.display().to_string(),
            reason: error.to_string(),
        })?;
        parse_usage_usec(&text).ok_or_else(|| ProbeError::PhysicalSource {
            source_name: "cgroup cpu.stat",
            path: self.path.display().to_string(),
            reason: "no `usage_usec` line".to_owned(),
        })
    }
}

/// Extracts `usage_usec <n>` from a `cpu.stat` body.
fn parse_usage_usec(text: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix("usage_usec "))
        .and_then(|value| value.trim().parse().ok())
}

/// `memory.current`.
#[derive(Debug, Clone)]
pub struct CgroupV2Memory {
    path: PathBuf,
}

impl CgroupV2Memory {
    /// Binds the reader to a path and proves the file is readable now.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError::PhysicalSource`] when the file cannot be read or
    /// does not hold a bare integer.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ProbeError> {
        let source = Self {
            path: path.as_ref().to_path_buf(),
        };
        source.task_memory_current()?;
        Ok(source)
    }
}

impl PhysicalMemorySource for CgroupV2Memory {
    fn task_memory_current(&self) -> Result<u64, ProbeError> {
        let text = fs::read_to_string(&self.path).map_err(|error| ProbeError::PhysicalSource {
            source_name: "cgroup memory.current",
            path: self.path.display().to_string(),
            reason: error.to_string(),
        })?;
        text.trim().parse().map_err(|_| ProbeError::PhysicalSource {
            source_name: "cgroup memory.current",
            path: self.path.display().to_string(),
            reason: format!("`{}` is not a bare integer", text.trim()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CgroupV2Cpu, CgroupV2Memory, CpuInstant, PhysicalCpuSource, PhysicalMemorySource,
        parse_usage_usec,
    };

    #[test]
    fn usage_usec_is_extracted_from_a_real_cpu_stat_body() {
        let body = "usage_usec 123456\nuser_usec 100000\nsystem_usec 23456\nnr_periods 0\n";
        assert_eq!(parse_usage_usec(body), Some(123_456));
    }

    #[test]
    fn a_cpu_stat_without_the_line_is_not_read_as_zero() {
        assert_eq!(parse_usage_usec("user_usec 1\nsystem_usec 2\n"), None);
        assert_eq!(parse_usage_usec(""), None);
        assert_eq!(parse_usage_usec("usage_usec not-a-number\n"), None);
    }

    #[test]
    fn a_missing_cgroup_file_is_a_startup_error_not_a_zero_reading() {
        let missing = std::env::temp_dir().join("aex-usage-probe-absent-cpu.stat");
        let opened = CgroupV2Cpu::open(&missing);
        assert!(opened.is_err(), "a missing cpu.stat must fail startup");

        let missing_memory = std::env::temp_dir().join("aex-usage-probe-absent-memory.current");
        assert!(CgroupV2Memory::open(&missing_memory).is_err());
    }

    #[test]
    fn a_readable_cgroup_file_binds_and_reads() {
        let dir = std::env::temp_dir().join("aex-usage-probe-clock-tests");
        std::fs::create_dir_all(&dir).expect("temp dir");

        let cpu_path = dir.join("cpu.stat");
        std::fs::write(&cpu_path, "usage_usec 4096\nuser_usec 1\n").expect("write");
        let cpu = CgroupV2Cpu::open(&cpu_path).expect("binds");
        assert_eq!(cpu.task_cpu_usec().expect("reads"), 4_096);

        let memory_path = dir.join("memory.current");
        std::fs::write(&memory_path, "8388608\n").expect("write");
        let memory = CgroupV2Memory::open(&memory_path).expect("binds");
        assert_eq!(memory.task_memory_current().expect("reads"), 8_388_608);

        std::fs::write(&memory_path, "not-a-number\n").expect("write");
        assert!(memory.task_memory_current().is_err());
    }

    #[test]
    fn a_backwards_thread_clock_delta_reads_as_zero_elapsed() {
        let earlier = CpuInstant::from_micros(500);
        let later = CpuInstant::from_micros(1_500);
        assert_eq!(later.saturating_since(earlier), 1_000);
        // Task migration between threads can move the reading backwards; that is
        // zero attributable elapsed, never an underflow into a huge charge.
        assert_eq!(earlier.saturating_since(later), 0);
    }
}
