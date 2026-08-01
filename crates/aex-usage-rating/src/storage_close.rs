//! Accepted `M-STOR-CLOSE` shadow accounting transformation.

/// Milliseconds in the storage meter's base minute.
pub const MINUTE_MILLISECONDS: u128 = 60_000;

/// Why a storage residence is being measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageCloseKind {
    /// An interior observation floors to the last complete minute.
    Interior,
    /// The final hard-delete observation ceils so no residence is lost.
    HardDelete,
}

/// Complete auditable shadow receipt for one rounded storage residence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowStorageClose {
    /// Resident bytes.
    pub bytes: u128,
    /// Exact observed duration.
    pub elapsed_milliseconds: u128,
    /// Direction used by the accepted transformation.
    pub kind: StorageCloseKind,
    /// Quantity emitted to the byte-minute meter.
    pub billed_byte_minutes: u128,
    /// Absolute difference from exact byte-milliseconds.
    pub rounding_error_byte_milliseconds: u128,
    /// Exclusive accepted bound: `bytes × one minute`.
    pub error_bound_byte_milliseconds: u128,
}

/// Applies the accepted shadow-only storage close transformation.
///
/// Interior observations floor; the final hard-delete observation ceils. The
/// returned receipt always carries the exact error and exclusive bound, so a
/// caller cannot emit a shadow close without reporting both.
///
/// # Errors
///
/// Rejects a zero-byte residence and every checked-arithmetic overflow.
pub fn shadow_storage_close(
    bytes: u128,
    elapsed_milliseconds: u128,
    kind: StorageCloseKind,
) -> Result<ShadowStorageClose, StorageCloseError> {
    if bytes == 0 {
        return Err(StorageCloseError::ZeroBytes);
    }
    let complete_minutes = elapsed_milliseconds / MINUTE_MILLISECONDS;
    let has_partial_minute = !elapsed_milliseconds.is_multiple_of(MINUTE_MILLISECONDS);
    let billed_minutes = if kind == StorageCloseKind::HardDelete && has_partial_minute {
        complete_minutes
            .checked_add(1)
            .ok_or(StorageCloseError::Overflow)?
    } else {
        complete_minutes
    };
    let billed_byte_minutes = bytes
        .checked_mul(billed_minutes)
        .ok_or(StorageCloseError::Overflow)?;
    let exact_byte_milliseconds = bytes
        .checked_mul(elapsed_milliseconds)
        .ok_or(StorageCloseError::Overflow)?;
    let billed_byte_milliseconds = billed_byte_minutes
        .checked_mul(MINUTE_MILLISECONDS)
        .ok_or(StorageCloseError::Overflow)?;
    let rounding_error_byte_milliseconds =
        exact_byte_milliseconds.abs_diff(billed_byte_milliseconds);
    let error_bound_byte_milliseconds = bytes
        .checked_mul(MINUTE_MILLISECONDS)
        .ok_or(StorageCloseError::Overflow)?;
    if rounding_error_byte_milliseconds >= error_bound_byte_milliseconds {
        return Err(StorageCloseError::BoundViolated);
    }
    Ok(ShadowStorageClose {
        bytes,
        elapsed_milliseconds,
        kind,
        billed_byte_minutes,
        rounding_error_byte_milliseconds,
        error_bound_byte_milliseconds,
    })
}

/// A rejected shadow storage close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StorageCloseError {
    /// A zero-size residence is not a meter segment.
    #[error("storage residence must contain at least one byte")]
    ZeroBytes,
    /// Exact or rounded quantity arithmetic exceeded `u128`.
    #[error("storage close arithmetic overflowed")]
    Overflow,
    /// The accepted `< bytes × one minute` bound did not hold.
    #[error("storage close rounding error exceeded its accepted bound")]
    BoundViolated,
}
