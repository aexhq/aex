//! Byte-stable archive writers.
//!
//! Packaging is not delegated to `cargo lambda --output-format zip`, `tar` or
//! `zip`, because artifact identity is the archive digest and those writers
//! embed the wall clock, the umask, the host operating system and directory
//! entries in an order nobody pinned. Every field below is written explicitly,
//! so two packagings of the same input closure produce one digest.
//!
//! Only the DEFLATE stream is borrowed. Its exact output is a function of the
//! compressor version, which is why the envelope records `packagerVersion`.

use std::io::Write as _;

use flate2::Compression;
use flate2::write::DeflateEncoder;

use crate::error::{Exit, Result, ToolError};

/// The fixed archive timestamp: the MS-DOS epoch, 1980-01-01T00:00:00Z.
///
/// ZIP cannot represent an earlier instant, so this is the only value that is
/// both legal and free of information about when the build ran.
pub const DOS_EPOCH_TIME: u16 = 0;
/// The MS-DOS date word for 1980-01-01.
pub const DOS_EPOCH_DATE: u16 = 0b0000_0000_0010_0001;
/// The fixed POSIX timestamp used by the tar writer and the gzip header.
pub const SOURCE_DATE_EPOCH_DEFAULT: u64 = 0;

/// One archive member.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Path inside the archive, `/`-separated, never absolute and never `..`.
    pub name: String,
    /// File contents.
    pub data: Vec<u8>,
    /// POSIX mode bits, without the file-type bits.
    pub mode: u32,
}

impl Entry {
    /// An executable member.
    #[must_use]
    pub fn executable(name: &str, data: Vec<u8>) -> Self {
        Self {
            name: name.to_owned(),
            data,
            mode: 0o755,
        }
    }

    /// A regular member.
    #[must_use]
    pub fn regular(name: &str, data: Vec<u8>) -> Self {
        Self {
            name: name.to_owned(),
            data,
            mode: 0o644,
        }
    }
}

fn check_names(entries: &[Entry]) -> Result<()> {
    for entry in entries {
        if entry.name.is_empty()
            || entry.name.starts_with('/')
            || entry.name.split('/').any(|segment| segment == "..")
            || entry.name.contains('\\')
        {
            return Err(ToolError::single(
                Exit::Usage,
                "archive-entry-name",
                format!(
                    "`{}` is not a safe archive member name; names are relative, \
                     `/`-separated and hold no `..`",
                    entry.name
                ),
            ));
        }
    }
    let mut names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    names.sort_unstable();
    let count = names.len();
    names.dedup();
    if names.len() != count {
        return Err(ToolError::single(
            Exit::Usage,
            "archive-entry-name",
            "the archive declares the same member twice",
        ));
    }
    Ok(())
}

/// Write a deterministic ZIP archive.
///
/// Entries are sorted by name, hold no directory records and no extra fields,
/// and carry the MS-DOS epoch as their modification time. Compression is
/// DEFLATE at a fixed level.
///
/// # Errors
/// Returns [`Exit::Usage`] when a member name is unsafe or duplicated, and
/// [`Exit::EnvelopeInvalid`] when the archive would exceed the 32-bit ZIP
/// limits this writer deliberately does not extend past.
pub fn write_zip(entries: &[Entry]) -> Result<Vec<u8>> {
    check_names(entries)?;
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|left, right| left.name.cmp(&right.name));

    let mut out: Vec<u8> = Vec::new();
    let mut directory: Vec<u8> = Vec::new();
    for entry in &sorted {
        let offset = u32::try_from(out.len()).map_err(|_| too_large())?;
        let mut crc = flate2::Crc::new();
        crc.update(&entry.data);
        let crc32 = crc.sum();
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::new(6));
        encoder
            .write_all(&entry.data)
            .and_then(|()| encoder.flush())
            .map_err(|err| ToolError::single(Exit::Usage, "archive-write", err.to_string()))?;
        let compressed = encoder
            .finish()
            .map_err(|err| ToolError::single(Exit::Usage, "archive-write", err.to_string()))?;
        let compressed_len = u32::try_from(compressed.len()).map_err(|_| too_large())?;
        let uncompressed_len = u32::try_from(entry.data.len()).map_err(|_| too_large())?;
        let name = entry.name.as_bytes();
        let name_len = u16::try_from(name.len()).map_err(|_| too_large())?;

        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes()); // local header
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // no flags: no data descriptor
        out.extend_from_slice(&8u16.to_le_bytes()); // DEFLATE
        out.extend_from_slice(&DOS_EPOCH_TIME.to_le_bytes());
        out.extend_from_slice(&DOS_EPOCH_DATE.to_le_bytes());
        out.extend_from_slice(&crc32.to_le_bytes());
        out.extend_from_slice(&compressed_len.to_le_bytes());
        out.extend_from_slice(&uncompressed_len.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // no extra field
        out.extend_from_slice(name);
        out.extend_from_slice(&compressed);

        directory.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        // Version made by: UNIX (3) with spec 2.0, so the external attributes
        // carry POSIX mode bits a Lambda unpacker will honour.
        directory.extend_from_slice(&0x0314u16.to_le_bytes());
        directory.extend_from_slice(&20u16.to_le_bytes());
        directory.extend_from_slice(&0u16.to_le_bytes());
        directory.extend_from_slice(&8u16.to_le_bytes());
        directory.extend_from_slice(&DOS_EPOCH_TIME.to_le_bytes());
        directory.extend_from_slice(&DOS_EPOCH_DATE.to_le_bytes());
        directory.extend_from_slice(&crc32.to_le_bytes());
        directory.extend_from_slice(&compressed_len.to_le_bytes());
        directory.extend_from_slice(&uncompressed_len.to_le_bytes());
        directory.extend_from_slice(&name_len.to_le_bytes());
        directory.extend_from_slice(&0u16.to_le_bytes()); // extra
        directory.extend_from_slice(&0u16.to_le_bytes()); // comment
        directory.extend_from_slice(&0u16.to_le_bytes()); // disk number
        directory.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        directory.extend_from_slice(&((0o100_000 | entry.mode) << 16).to_le_bytes());
        directory.extend_from_slice(&offset.to_le_bytes());
        directory.extend_from_slice(name);
    }

    let directory_offset = u32::try_from(out.len()).map_err(|_| too_large())?;
    let directory_size = u32::try_from(directory.len()).map_err(|_| too_large())?;
    let count = u16::try_from(sorted.len()).map_err(|_| too_large())?;
    out.extend_from_slice(&directory);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // this disk
    out.extend_from_slice(&0u16.to_le_bytes()); // directory start disk
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&directory_size.to_le_bytes());
    out.extend_from_slice(&directory_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // no archive comment
    Ok(out)
}

fn too_large() -> ToolError {
    ToolError::single(
        Exit::EnvelopeInvalid,
        "archive-too-large",
        "the archive exceeds the 32-bit ZIP limits; ZIP64 is deliberately not written \
         because it would change the digest of every existing artifact",
    )
}

/// Write a deterministic `ustar` tar archive.
///
/// Entries are sorted, owner and group are numeric zero with empty names, and
/// every timestamp is `source_date_epoch`.
///
/// # Errors
/// Returns [`Exit::Usage`] when a member name is unsafe, duplicated, or longer
/// than the 100-byte `ustar` name field.
pub fn write_tar(entries: &[Entry], source_date_epoch: u64) -> Result<Vec<u8>> {
    check_names(entries)?;
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|left, right| left.name.cmp(&right.name));

    let mut out = Vec::new();
    for entry in sorted {
        if entry.name.len() > 100 {
            return Err(ToolError::single(
                Exit::Usage,
                "archive-entry-name",
                format!(
                    "`{}` exceeds the 100-byte ustar name field; a PAX extension would \
                     add a second timestamp source",
                    entry.name
                ),
            ));
        }
        let mut header = [0u8; 512];
        header[..entry.name.len()].copy_from_slice(entry.name.as_bytes());
        write_octal(&mut header[100..108], u64::from(entry.mode), 7);
        write_octal(&mut header[108..116], 0, 7); // uid
        write_octal(&mut header[116..124], 0, 7); // gid
        write_octal(&mut header[124..136], entry.data.len() as u64, 11);
        write_octal(&mut header[136..148], source_date_epoch, 11);
        header[148..156].fill(b' '); // checksum placeholder
        header[156] = b'0'; // regular file
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        write_octal(&mut header[329..337], 0, 7); // devmajor
        write_octal(&mut header[337..345], 0, 7); // devminor
        let checksum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
        write_octal(&mut header[148..154], u64::from(checksum), 6);
        header[154] = 0;
        header[155] = b' ';
        out.extend_from_slice(&header);
        out.extend_from_slice(&entry.data);
        let padding = (512 - entry.data.len() % 512) % 512;
        out.extend(std::iter::repeat_n(0u8, padding));
    }
    out.extend(std::iter::repeat_n(0u8, 1024)); // two zero blocks
    Ok(out)
}

/// Write a deterministic gzip stream: no filename, no comment, zero mtime and
/// the "unknown" operating system byte, so nothing about the build host leaks
/// into the digest.
///
/// # Errors
/// Returns [`Exit::Usage`] when compression fails.
pub fn gzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = flate2::GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(Vec::new(), Compression::new(6));
    encoder
        .write_all(data)
        .map_err(|err| ToolError::single(Exit::Usage, "archive-write", err.to_string()))?;
    encoder
        .finish()
        .map_err(|err| ToolError::single(Exit::Usage, "archive-write", err.to_string()))
}

/// Write a deterministic `.tar.gz`.
///
/// # Errors
/// Propagates tar and gzip failures.
pub fn write_tar_gz(entries: &[Entry], source_date_epoch: u64) -> Result<Vec<u8>> {
    gzip(&write_tar(entries, source_date_epoch)?)
}

fn write_octal(field: &mut [u8], value: u64, digits: usize) {
    let rendered = format!("{value:0digits$o}");
    let bytes = rendered.as_bytes();
    let start = field.len().saturating_sub(bytes.len() + 1);
    field[start..start + bytes.len()].copy_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::{Entry, write_tar, write_tar_gz, write_zip};
    use crate::canon::digest_bytes;

    #[test]
    fn two_packagings_of_the_same_input_produce_one_digest() {
        let entries = vec![Entry::executable("bootstrap", b"ELF...payload".to_vec())];
        let first = write_zip(&entries).unwrap();
        let second = write_zip(&entries).unwrap();
        assert_eq!(digest_bytes(&first), digest_bytes(&second));
    }

    #[test]
    fn the_lambda_archive_holds_exactly_one_entry_named_bootstrap_at_mode_0755() {
        let archive = write_zip(&[Entry::executable("bootstrap", b"payload".to_vec())]).unwrap();
        // End of central directory: two entry counts of 1.
        let eocd = archive.len() - 22;
        assert_eq!(&archive[eocd..eocd + 4], &0x0605_4b50u32.to_le_bytes());
        assert_eq!(
            u16::from_le_bytes([archive[eocd + 8], archive[eocd + 9]]),
            1,
            "exactly one member"
        );
        // Local header name.
        let name_len = u16::from_le_bytes([archive[26], archive[27]]) as usize;
        assert_eq!(&archive[30..30 + name_len], b"bootstrap");
        assert_eq!(
            u16::from_le_bytes([archive[28], archive[29]]),
            0,
            "no extra field"
        );
        // Central directory external attributes carry the mode.
        let directory_offset =
            u32::from_le_bytes(archive[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
        let external = u32::from_le_bytes(
            archive[directory_offset + 38..directory_offset + 42]
                .try_into()
                .unwrap(),
        );
        assert_eq!(external >> 16, 0o100_755);
    }

    #[test]
    fn the_archive_timestamp_is_the_dos_epoch_and_not_the_wall_clock() {
        let archive = write_zip(&[Entry::executable("bootstrap", b"x".to_vec())]).unwrap();
        assert_eq!(u16::from_le_bytes([archive[10], archive[11]]), 0);
        assert_eq!(u16::from_le_bytes([archive[12], archive[13]]), 0x0021);
    }

    #[test]
    fn entry_order_does_not_depend_on_the_caller() {
        let forward = write_zip(&[
            Entry::regular("a.txt", b"a".to_vec()),
            Entry::regular("b.txt", b"b".to_vec()),
        ])
        .unwrap();
        let reversed = write_zip(&[
            Entry::regular("b.txt", b"b".to_vec()),
            Entry::regular("a.txt", b"a".to_vec()),
        ])
        .unwrap();
        assert_eq!(digest_bytes(&forward), digest_bytes(&reversed));
    }

    #[test]
    fn a_traversal_member_name_is_rejected() {
        let err = write_zip(&[Entry::regular("../escape", b"x".to_vec())]).unwrap_err();
        assert_eq!(err.rules(), vec!["archive-entry-name"]);
        let err = write_zip(&[Entry::regular("/abs", b"x".to_vec())]).unwrap_err();
        assert_eq!(err.rules(), vec!["archive-entry-name"]);
    }

    #[test]
    fn a_duplicated_member_name_is_rejected() {
        let err = write_zip(&[
            Entry::regular("a", b"1".to_vec()),
            Entry::regular("a", b"2".to_vec()),
        ])
        .unwrap_err();
        assert_eq!(err.rules(), vec!["archive-entry-name"]);
    }

    #[test]
    fn tar_headers_carry_a_valid_checksum_and_a_fixed_timestamp() {
        let archive = write_tar(&[Entry::regular("f", b"data".to_vec())], 0).unwrap();
        assert_eq!(&archive[257..263], b"ustar\0");
        let mut recomputed: u32 = 0;
        for (index, byte) in archive[..512].iter().enumerate() {
            recomputed += if (148..156).contains(&index) {
                u32::from(b' ')
            } else {
                u32::from(*byte)
            };
        }
        let stored = std::str::from_utf8(&archive[148..154]).unwrap();
        assert_eq!(u32::from_str_radix(stored.trim(), 8).unwrap(), recomputed);
        let mtime = std::str::from_utf8(&archive[136..147]).unwrap();
        assert_eq!(u64::from_str_radix(mtime.trim(), 8).unwrap(), 0);
    }

    #[test]
    fn tar_gz_is_byte_stable() {
        let entries = vec![Entry::regular("index.html", b"<!doctype html>".to_vec())];
        assert_eq!(
            digest_bytes(&write_tar_gz(&entries, 0).unwrap()),
            digest_bytes(&write_tar_gz(&entries, 0).unwrap())
        );
    }

    #[test]
    fn tar_rejects_a_name_longer_than_the_ustar_field() {
        let long = "a".repeat(101);
        let err = write_tar(&[Entry::regular(&long, Vec::new())], 0).unwrap_err();
        assert_eq!(err.rules(), vec!["archive-entry-name"]);
    }
}
