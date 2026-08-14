//! Fail-fast configuration primitives shared by every regional deployable.
//!
//! Nothing here has a default. A variable that identifies a resource — a table,
//! a bucket, a queue, a KMS key, a region, a plane — must be supplied
//! explicitly, because a defaulted resource identifier silently binds a process
//! to the wrong plane and the mistake is only visible in the data it writes.
//!
//! Every refusal names the variable. A start-up failure that says "invalid
//! configuration" costs an operator a bisect; one that says
//! `AEX_SESSION_AUTHORITY_TABLE is missing` costs them nothing.

use aex_identity_domain::assertion::Plane;
use aex_wire::types::Region;

/// Reads one environment variable.
///
/// A composition root passes `std::env::var`-backed lookup; a test passes a map,
/// because `std::env::set_var` is `unsafe` in edition 2024 and this workspace
/// forbids `unsafe` code.
pub trait Lookup {
    /// The value bound to `name`, if any.
    fn get(&self, name: &str) -> Option<String>;
}

impl<F> Lookup for F
where
    F: Fn(&str) -> Option<String>,
{
    fn get(&self, name: &str) -> Option<String> {
        self(name)
    }
}

/// The process environment.
#[derive(Debug, Clone, Copy, Default)]
pub struct Environment;

impl Lookup for Environment {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// Why a deployable refused to start.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegionalHttpConfigError {
    /// A required variable was absent or blank.
    #[error("required environment variable `{name}` is missing")]
    Missing {
        /// The variable that must be supplied.
        name: &'static str,
    },
    /// A required variable was present but unusable.
    #[error("environment variable `{name}` is invalid: {reason}")]
    Invalid {
        /// The variable that was rejected.
        name: &'static str,
        /// Why the supplied value was rejected.
        reason: String,
    },
    /// A variable this deployable must never hold was present.
    ///
    /// This is the configuration half of capability admission: a binary that
    /// cannot be allowed to reach a resource refuses to start when the resource
    /// is bound to it, whatever IAM says.
    #[error("environment variable `{name}` is forbidden for `{deployable}`: {reason}")]
    Forbidden {
        /// The variable that must not be set.
        name: &'static str,
        /// The deployable that refused it.
        deployable: &'static str,
        /// Why it is forbidden.
        reason: &'static str,
    },
}

/// A non-blank value.
///
/// # Errors
///
/// Returns [`RegionalHttpConfigError::Missing`] when the variable is absent or blank.
pub fn required<L: Lookup + ?Sized>(
    lookup: &L,
    name: &'static str,
) -> Result<String, RegionalHttpConfigError> {
    match lookup.get(name) {
        Some(value) if !value.trim().is_empty() => Ok(value.trim().to_owned()),
        _ => Err(RegionalHttpConfigError::Missing { name }),
    }
}

/// An optional non-blank value. A blank value is absent, never empty-but-present.
pub fn optional<L: Lookup + ?Sized>(lookup: &L, name: &'static str) -> Option<String> {
    lookup
        .get(name)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Refuses a variable this deployable may never be bound to.
///
/// # Errors
///
/// Returns [`RegionalHttpConfigError::Forbidden`] when the variable is set at all.
pub fn forbidden<L: Lookup + ?Sized>(
    lookup: &L,
    name: &'static str,
    deployable: &'static str,
    reason: &'static str,
) -> Result<(), RegionalHttpConfigError> {
    if lookup.get(name).is_some() {
        return Err(RegionalHttpConfigError::Forbidden {
            name,
            deployable,
            reason,
        });
    }
    Ok(())
}

/// A positive integer.
///
/// # Errors
///
/// Returns [`RegionalHttpConfigError::Missing`] or [`RegionalHttpConfigError::Invalid`]; zero is
/// rejected, because a zero page, lease or shard count is never a value anyone
/// meant to deploy.
pub fn positive_u64<L: Lookup + ?Sized>(
    lookup: &L,
    name: &'static str,
) -> Result<u64, RegionalHttpConfigError> {
    let raw = required(lookup, name)?;
    raw.parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| RegionalHttpConfigError::Invalid {
            name,
            reason: format!("expected a positive integer, got `{raw}`"),
        })
}

/// A positive integer inside an inclusive range.
///
/// # Errors
///
/// As [`positive_u64`], plus [`RegionalHttpConfigError::Invalid`] outside the range.
pub fn bounded_u64<L: Lookup + ?Sized>(
    lookup: &L,
    name: &'static str,
    min: u64,
    max: u64,
) -> Result<u64, RegionalHttpConfigError> {
    let value = positive_u64(lookup, name)?;
    if value < min || value > max {
        return Err(RegionalHttpConfigError::Invalid {
            name,
            reason: format!("expected {min}..={max}, got `{value}`"),
        });
    }
    Ok(value)
}

/// A positive `usize` inside an inclusive range.
///
/// # Errors
///
/// As [`bounded_u64`].
pub fn bounded_usize<L: Lookup + ?Sized>(
    lookup: &L,
    name: &'static str,
    min: u64,
    max: u64,
) -> Result<usize, RegionalHttpConfigError> {
    let value = bounded_u64(lookup, name, min, max)?;
    usize::try_from(value).map_err(|_| RegionalHttpConfigError::Invalid {
        name,
        reason: format!("`{value}` does not fit this host's address width"),
    })
}

/// The deployment plane.
///
/// There is no third plane and no default: a defaulted plane binds a process to
/// the wrong environment's resources without any other symptom.
///
/// It resolves to the typed [`Plane`] the assertion envelope binds rather than
/// to a validated string, so the plane a process reports in telemetry and the
/// plane it will accept an assertion for are the same value. `Plane::as_str`
/// gives the verbatim spelling back for the encryption context.
///
/// # Errors
///
/// Returns [`RegionalHttpConfigError::Invalid`] for anything but `dev` or `prd`.
pub fn plane_name<L: Lookup + ?Sized>(
    lookup: &L,
    name: &'static str,
) -> Result<Plane, RegionalHttpConfigError> {
    let raw = required(lookup, name)?;
    Plane::parse(&raw).ok_or_else(|| RegionalHttpConfigError::Invalid {
        name,
        reason: format!("expected `dev` or `prd`, got `{raw}`"),
    })
}

/// A region this platform is enabled in.
///
/// # Errors
///
/// Returns [`RegionalHttpConfigError::Invalid`] for a region outside the closed set.
pub fn region<L: Lookup + ?Sized>(
    lookup: &L,
    name: &'static str,
) -> Result<Region, RegionalHttpConfigError> {
    let raw = required(lookup, name)?;
    Region::ALL
        .iter()
        .copied()
        .find(|candidate| candidate.as_str() == raw)
        .ok_or_else(|| RegionalHttpConfigError::Invalid {
            name,
            reason: format!("`{raw}` is not an enabled region"),
        })
}

/// An `ARN` whose partition, service, region and account are all present, and
/// whose region matches the plane binding.
///
/// A resource in another region is the failure mode this check exists for: it
/// passes every type check, deploys cleanly, and silently writes a tenant's data
/// outside its declared residency.
///
/// # Errors
///
/// Returns [`RegionalHttpConfigError::Invalid`] for a malformed `ARN` or a region mismatch.
pub fn arn_in_region<L: Lookup + ?Sized>(
    lookup: &L,
    name: &'static str,
    expected: Region,
    service: &'static str,
) -> Result<Arn, RegionalHttpConfigError> {
    let raw = required(lookup, name)?;
    let arn = Arn::parse(&raw).map_err(|reason| RegionalHttpConfigError::Invalid {
        name,
        reason: reason.to_owned(),
    })?;
    if arn.service != service {
        return Err(RegionalHttpConfigError::Invalid {
            name,
            reason: format!("expected an `{service}` ARN, got `{}`", arn.service),
        });
    }
    if arn.region != expected.as_str() {
        return Err(RegionalHttpConfigError::Invalid {
            name,
            reason: format!(
                "resource is in `{}` but this process is bound to `{}`",
                arn.region,
                expected.as_str()
            ),
        });
    }
    Ok(arn)
}

/// The parsed parts of an `ARN` a composition root binds to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arn {
    /// The whole `ARN`, verbatim.
    pub value: String,
    /// The `AWS` partition.
    pub partition: String,
    /// The service namespace.
    pub service: String,
    /// The region, which must equal the plane binding.
    pub region: String,
    /// The owning account, which becomes the expected bucket/table owner.
    pub account: String,
    /// Everything after the account.
    pub resource: String,
}

impl Arn {
    /// Parses `arn:partition:service:region:account:resource`.
    ///
    /// # Errors
    ///
    /// Returns the reason the value is not a fully qualified `ARN`.
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        let mut parts = value.splitn(6, ':');
        let (Some("arn"), Some(partition), Some(service), Some(region), Some(account)) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return Err("expected `arn:partition:service:region:account:resource`");
        };
        let resource = parts.next().ok_or("the ARN names no resource")?;
        if partition.is_empty()
            || service.is_empty()
            || region.is_empty()
            || account.is_empty()
            || resource.is_empty()
        {
            return Err("every ARN field must be present; a wildcard is not a binding");
        }
        Ok(Self {
            value: value.to_owned(),
            partition: partition.to_owned(),
            service: service.to_owned(),
            region: region.to_owned(),
            account: account.to_owned(),
            resource: resource.to_owned(),
        })
    }
}

/// An `SQS` queue URL whose region matches the plane binding.
///
/// # Errors
///
/// Returns [`RegionalHttpConfigError::Invalid`] for a malformed URL or a region mismatch.
pub fn queue_url<L: Lookup + ?Sized>(
    lookup: &L,
    name: &'static str,
    expected: Region,
) -> Result<String, RegionalHttpConfigError> {
    let raw = required(lookup, name)?;
    let host = raw
        .strip_prefix("https://sqs.")
        .and_then(|rest| rest.split_once('.'))
        .map(|(region, _)| region)
        .ok_or_else(|| RegionalHttpConfigError::Invalid {
            name,
            reason: format!("expected `https://sqs.<region>.amazonaws.com/...`, got `{raw}`"),
        })?;
    if host != expected.as_str() {
        return Err(RegionalHttpConfigError::Invalid {
            name,
            reason: format!(
                "queue is in `{host}` but this process is bound to `{}`",
                expected.as_str()
            ),
        });
    }
    Ok(raw)
}

/// A value from a closed set.
///
/// # Errors
///
/// Returns [`RegionalHttpConfigError::Invalid`] for a value outside the set, naming the set.
pub fn one_of<L: Lookup + ?Sized>(
    lookup: &L,
    name: &'static str,
    permitted: &[&'static str],
) -> Result<String, RegionalHttpConfigError> {
    let raw = required(lookup, name)?;
    if permitted.contains(&raw.as_str()) {
        Ok(raw)
    } else {
        Err(RegionalHttpConfigError::Invalid {
            name,
            reason: format!("expected one of {permitted:?}, got `{raw}`"),
        })
    }
}
