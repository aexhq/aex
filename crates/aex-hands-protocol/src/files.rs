//! Binary-safe, exact-generation live-workspace file transfer messages.
//!
//! The HTTP frame preamble carries the generation and fence. These payloads
//! therefore carry only the file intent and its bounded bytes; accepting one
//! through an unbound or stale guest is impossible before this enum is decoded.

use aex_wire::ids::{ContentHash, Uuid7};
use serde::{Deserialize, Serialize};

use crate::operation::{ByteRangeRequest, FileMode, GuestPath};

/// Largest binary body carried by one authenticated guest frame.
///
/// Base64 expands by four thirds. Seven hundred thousand bytes plus the closed
/// JSON envelope stays below the one-MiB Hands frame ceiling.
pub const FILE_FRAME_BYTES: u32 = 700_000;

/// Fixed public multipart part size, except for the final part.
///
/// Four MiB remains within a six-MiB synchronous Lambda payload even when an
/// HTTP adapter base64-encodes the request. The trusted service decomposes it
/// into [`FILE_FRAME_BYTES`] guest calls.
pub const FILE_TRANSFER_PART_BYTES: u32 = 4 * 1024 * 1024;

/// Largest complete live file admitted by this protocol.
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// One caller-minted, generation-local upload identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FileUploadId(pub Uuid7);

impl std::fmt::Display for FileUploadId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// One caller-minted, generation-local download identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FileDownloadId(pub Uuid7);

impl std::fmt::Display for FileDownloadId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// One completed upload part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FilePartReceipt {
    /// One-based part number.
    pub part_number: u32,
    /// First byte of this part in the complete file.
    pub offset: u64,
    /// Exact number of bytes retained for the part.
    pub size_bytes: u32,
    /// SHA-256 of this part.
    pub sha256: ContentHash,
}

/// The resumable state of one generation-local upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FileUploadState {
    /// Caller-minted upload identity.
    pub upload: FileUploadId,
    /// Final workspace path.
    pub path: GuestPath,
    /// Declared complete length.
    pub size_bytes: u64,
    /// Declared complete SHA-256.
    pub sha256: ContentHash,
    /// Parts already durably retained, ascending by part number.
    pub parts: Vec<FilePartReceipt>,
    /// Whether the final atomic rename has completed.
    pub complete: bool,
}

/// One opened download range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FileDownloadState {
    /// Caller-minted transfer identity.
    pub download: FileDownloadId,
    /// Whole-file length observed when the descriptor was opened.
    pub file_size_bytes: u64,
    /// First selected byte, inclusive.
    pub start: u64,
    /// Number of selected bytes. Zero is the valid whole range of an empty
    /// file; an explicit range over an empty file is refused.
    pub length_bytes: u64,
    /// SHA-256 of the complete file held by the opened descriptor.
    pub sha256: ContentHash,
    /// Opaque identity binding file bytes and bounded stat evidence.
    pub version: ContentHash,
}

/// One chunk from an opened download descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FileDownloadChunk {
    /// Transfer identity.
    pub download: FileDownloadId,
    /// Absolute file offset of the first byte.
    pub offset: u64,
    /// Binary bytes, base64 on the JSON wire.
    #[serde(with = "base64_bytes")]
    pub bytes: Vec<u8>,
    /// SHA-256 of `bytes` alone.
    pub sha256: ContentHash,
    /// Whether this reaches the selected range's end.
    pub last: bool,
}

/// One binary-safe live-file RPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "fileOperation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum FileRequest {
    /// Create or reopen a generation-local multipart upload.
    UploadOpen {
        /// Caller-minted upload identity.
        upload: FileUploadId,
        /// Final workspace path.
        path: GuestPath,
        /// Complete length.
        size_bytes: u64,
        /// Complete SHA-256.
        sha256: ContentHash,
        /// Final file mode.
        mode: FileMode,
    },
    /// Read the durably retained parts of an upload.
    UploadStatus {
        /// Upload identity.
        upload: FileUploadId,
    },
    /// Create or reopen one logical public upload part.
    UploadPartOpen {
        /// Upload identity.
        upload: FileUploadId,
        /// One-based part number.
        part_number: u32,
        /// First byte in the complete file.
        offset: u64,
        /// Exact size of this logical part.
        size_bytes: u32,
        /// SHA-256 of the complete logical part.
        sha256: ContentHash,
    },
    /// Put one bounded frame of an open logical part atomically.
    UploadPartChunk {
        /// Upload identity.
        upload: FileUploadId,
        /// One-based logical part number.
        part_number: u32,
        /// Offset within the logical part.
        chunk_offset: u32,
        /// SHA-256 of `bytes`.
        sha256: ContentHash,
        /// Binary bytes, base64 on the authenticated JSON wire.
        #[serde(with = "base64_bytes")]
        bytes: Vec<u8>,
    },
    /// Verify and publish one logical part into the resumable upload.
    UploadPartComplete {
        /// Upload identity.
        upload: FileUploadId,
        /// One-based logical part number.
        part_number: u32,
    },
    /// Verify every part and publish the final file by one atomic rename.
    UploadComplete {
        /// Upload identity.
        upload: FileUploadId,
    },
    /// Forget a partial upload and its temporary bytes.
    UploadAbort {
        /// Upload identity.
        upload: FileUploadId,
    },
    /// Open one regular file without following a symlink.
    DownloadOpen {
        /// Caller-minted transfer identity.
        download: FileDownloadId,
        /// Workspace path.
        path: GuestPath,
        /// Selected range; absent means the whole file.
        range: Option<ByteRangeRequest>,
    },
    /// Read a bounded chunk from an already-open file descriptor.
    DownloadChunk {
        /// Transfer identity.
        download: FileDownloadId,
        /// Absolute file offset expected next.
        offset: u64,
        /// Requested bytes, at most [`FILE_FRAME_BYTES`].
        max_bytes: u32,
    },
    /// Close the descriptor early.
    DownloadClose {
        /// Transfer identity.
        download: FileDownloadId,
    },
}

/// One live-file answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "fileResponse",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum FileResponse {
    /// Upload open/status/part answer.
    Upload {
        /// Current resumable state.
        state: FileUploadState,
    },
    /// Upload reached its final atomic path.
    UploadComplete {
        /// Final state.
        state: FileUploadState,
    },
    /// Partial state was removed.
    UploadAborted {
        /// Removed upload identity.
        upload: FileUploadId,
    },
    /// Download descriptor opened.
    DownloadOpened {
        /// Pinned descriptor.
        state: FileDownloadState,
    },
    /// One download chunk.
    DownloadChunk {
        /// Returned chunk.
        chunk: FileDownloadChunk,
    },
    /// Download descriptor closed.
    DownloadClosed {
        /// Closed transfer identity.
        download: FileDownloadId,
        /// Reverified complete-file SHA-256.
        sha256: ContentHash,
        /// Reverified version token.
        version: ContentHash,
    },
    /// A stable guest-side refusal. The trusted caller maps it onto a declared
    /// public error; no customer-controlled diagnostic is used as authority.
    Rejected {
        /// Closed reason code.
        code: FileFailureCode,
    },
}

/// Closed live-file refusal vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileFailureCode {
    /// The path or transfer does not exist.
    NotFound,
    /// The request conflicts with existing upload state.
    Conflict,
    /// A path is a directory or a symlink, or traverses one.
    InvalidPath,
    /// A range, offset, length, part number or checksum is invalid.
    InvalidRequest,
    /// The fixed file, part or workspace bound was exceeded.
    LimitExceeded,
    /// The guest filesystem could not complete the request.
    Unavailable,
}

mod base64_bytes {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize as _, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(value))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        use serde::de::Error as _;
        let raw = String::deserialize(deserializer)?;
        STANDARD.decode(raw.as_bytes()).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::{FILE_FRAME_BYTES, FileDownloadId, FileRequest, FileUploadId};
    use crate::operation::GuestRoot;
    use aex_wire::ids::{ContentHash, Uuid7};

    #[test]
    fn the_largest_part_stays_inside_the_authenticated_frame() {
        let request = FileRequest::UploadPartChunk {
            upload: FileUploadId(Uuid7::compose(1, [1; 10])),
            part_number: 1,
            chunk_offset: 0,
            sha256: ContentHash::from_bytes([2; 32]),
            bytes: vec![3; FILE_FRAME_BYTES as usize],
        };
        let encoded = serde_json::to_vec(&request).expect("file request encodes");
        assert!(encoded.len() < 1_048_576, "encoded {} bytes", encoded.len());

        let decoded: FileRequest = serde_json::from_slice(&encoded).expect("it decodes");
        assert_eq!(decoded, request);
        let _ = (
            FileDownloadId(Uuid7::compose(1, [4; 10])),
            GuestRoot::workspace(),
        );
    }
}
