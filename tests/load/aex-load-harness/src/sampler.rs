//! The resource sampler.
//!
//! `LEAK-SOAK-12H` and `LOAD-200-SAFE` are assertions about resident set size,
//! file descriptors and task counts over time, so those counters have to come
//! from somewhere trustworthy. A sampler that cannot read a counter on this
//! platform returns [`SamplerError::Unsupported`] - it never reports zero,
//! because a flat line of zeroes looks exactly like a system with no leak.

use serde::{Deserialize, Serialize};

/// Why a sample could not be taken.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SamplerError {
    /// This platform does not expose the counter.
    #[error(
        "`{counter}` cannot be sampled on {platform}; a load gate that asserts it must run on a platform that exposes it"
    )]
    Unsupported {
        /// Which counter was asked for.
        counter: &'static str,
        /// The platform that cannot answer.
        platform: &'static str,
    },
    /// The counter exists but could not be read.
    #[error("cannot read `{path}`: {detail}")]
    Read {
        /// The source that failed.
        path: String,
        /// What went wrong.
        detail: String,
    },
    /// The counter was read but not understood.
    #[error("`{path}` did not contain `{field}` in the expected form")]
    Parse {
        /// The source that was read.
        path: String,
        /// The field that was missing or malformed.
        field: &'static str,
    },
}

/// One observation of the process's resource use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sample {
    /// Resident set size in bytes.
    pub rss_bytes: u64,
    /// Open file descriptors.
    pub open_fds: u64,
}

/// Something that can observe the system under test's resource use.
pub trait ResourceProbe: std::fmt::Debug {
    /// Takes one sample.
    ///
    /// # Errors
    ///
    /// Returns [`SamplerError`] when the counters cannot be read on this
    /// platform, rather than a zeroed sample.
    fn sample(&self) -> Result<Sample, SamplerError>;
}

/// A probe that reads the Linux `/proc` filesystem.
///
/// The load and soak lanes run against Linux artifacts, so this is the probe
/// that matters. On any other platform it fails loudly, which is why a
/// developer machine cannot accidentally produce a soak receipt.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcProbe;

impl ProcProbe {
    /// A probe for the current process.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Parses `VmRSS` out of a `/proc/<pid>/status` document.
    ///
    /// # Errors
    ///
    /// Returns [`SamplerError::Parse`] when the field is absent or malformed.
    pub fn parse_vm_rss(status: &str, path: &str) -> Result<u64, SamplerError> {
        for line in status.lines() {
            let Some(rest) = line.strip_prefix("VmRSS:") else {
                continue;
            };
            let mut parts = rest.split_whitespace();
            let value = parts.next().and_then(|text| text.parse::<u64>().ok());
            let unit = parts.next();
            if let (Some(value), Some("kB")) = (value, unit) {
                return Ok(value * 1024);
            }
            return Err(SamplerError::Parse {
                path: path.to_owned(),
                field: "VmRSS",
            });
        }
        Err(SamplerError::Parse {
            path: path.to_owned(),
            field: "VmRSS",
        })
    }
}

impl ResourceProbe for ProcProbe {
    fn sample(&self) -> Result<Sample, SamplerError> {
        if !cfg!(target_os = "linux") {
            return Err(SamplerError::Unsupported {
                counter: "rss_bytes",
                platform: std::env::consts::OS,
            });
        }
        let status_path = "/proc/self/status";
        let status = std::fs::read_to_string(status_path).map_err(|error| SamplerError::Read {
            path: status_path.to_owned(),
            detail: error.to_string(),
        })?;
        let rss_bytes = Self::parse_vm_rss(&status, status_path)?;
        let fd_path = "/proc/self/fd";
        let open_fds = std::fs::read_dir(fd_path)
            .map_err(|error| SamplerError::Read {
                path: fd_path.to_owned(),
                detail: error.to_string(),
            })?
            .count();
        Ok(Sample {
            rss_bytes,
            open_fds: open_fds as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ProcProbe, ResourceProbe, SamplerError};

    const STATUS: &str = "Name:\taex\nVmPeak:\t 123456 kB\nVmRSS:\t   2048 kB\nThreads:\t8\n";

    #[test]
    fn vm_rss_is_read_in_bytes() {
        assert_eq!(
            ProcProbe::parse_vm_rss(STATUS, "/proc/self/status").expect("VmRSS is present"),
            2048 * 1024
        );
    }

    #[test]
    fn a_missing_field_names_the_field_rather_than_returning_zero() {
        let error = ProcProbe::parse_vm_rss("Name:\taex\n", "/proc/self/status")
            .expect_err("VmRSS is absent");
        assert_eq!(
            error,
            SamplerError::Parse {
                path: "/proc/self/status".to_owned(),
                field: "VmRSS"
            }
        );
    }

    #[test]
    fn a_malformed_unit_is_a_parse_failure() {
        let error = ProcProbe::parse_vm_rss("VmRSS:\t2048 pages\n", "/proc/self/status")
            .expect_err("the unit is not kB");
        assert!(matches!(error, SamplerError::Parse { .. }), "{error}");
    }

    #[test]
    fn sampling_off_linux_is_an_error_and_never_a_zeroed_sample() {
        let result = ProcProbe::new().sample();
        if cfg!(target_os = "linux") {
            let sample = result.expect("a Linux host exposes /proc");
            assert!(sample.rss_bytes > 0);
        } else {
            let error = result.expect_err("only Linux exposes /proc");
            assert!(matches!(error, SamplerError::Unsupported { .. }), "{error}");
        }
    }
}
