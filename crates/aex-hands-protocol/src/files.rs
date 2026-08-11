//! Binary-safe, exact-generation live-workspace file transfer messages.
//!
//! The HTTP frame preamble carries the generation and fence. These payloads
//! therefore carry only the file intent and its bounded bytes; accepting one
//! through an unbound or stale guest is impossible before this enum is decoded.

use aex_wire::ids::{ContentHash, Uuid7};
use serde::{Deserialize, Serialize};

use crate::operation::{ByteRangeRequest, FileMode, GuestPath};

/// Largest live-directory page the guest will produce.
pub const MAX_FILE_LIST_ENTRIES: u32 = 1_000;

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

/// Whether one complete live file fits the protocol ceiling.
///
/// Kept beside [`MAX_FILE_BYTES`] so every Rust-side admission path shares the
/// same inclusive boundary.
#[must_use]
pub const fn live_file_size_is_admitted(size_bytes: u64) -> bool {
    size_bytes <= MAX_FILE_BYTES
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Fixed public ranges and their expected SHA-256 values, ascending.
    pub parts: Vec<FilePartReceipt>,
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

/// What one live `lstat` observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveFileEntryKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link, reported without following it.
    Symlink,
}

/// One structured live entry. No content digest is computed or implied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LiveFileEntry {
    /// Absolute normalized path inside the guest workspace.
    pub path: GuestPath,
    /// Entry kind from `lstat`.
    pub kind: LiveFileEntryKind,
    /// Size reported by `lstat`.
    pub size_bytes: u64,
    /// Modification time in Unix epoch milliseconds.
    pub mtime_ms: i64,
    /// POSIX mode bits.
    pub mode: u32,
    /// Exact link text, when this is a symlink. It is never resolved or followed.
    pub target: Option<String>,
}

/// One ordered live-directory page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LiveFileListing {
    /// Entries in strict path order.
    pub entries: Vec<LiveFileEntry>,
    /// Resume strictly after this path, when the page was truncated.
    pub next_after: Option<GuestPath>,
}

/// One binary-safe live-file RPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "fileOperation",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum FileRequest {
    /// List one live directory from `lstat` metadata only.
    List {
        /// Directory to list.
        path: GuestPath,
        /// Whether to descend through directories. Symlinks are never followed.
        recursive: bool,
        /// Page size, at most [`MAX_FILE_LIST_ENTRIES`].
        limit: u32,
        /// Resume strictly after this path.
        after: Option<GuestPath>,
    },
    /// Stat one path without following a symlink.
    Stat {
        /// Path to observe.
        path: GuestPath,
    },
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
    /// Close an abandoned descriptor without an expensive final hash pass.
    DownloadAbort {
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
    /// One live-directory page.
    Listing {
        /// Structured lstat-only answer.
        listing: LiveFileListing,
    },
    /// One lstat-only live entry.
    Entry {
        /// Structured answer.
        entry: LiveFileEntry,
    },
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
    /// An abandoned descriptor was closed.
    DownloadAborted {
        /// Closed transfer identity.
        download: FileDownloadId,
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

pub(crate) mod base64_bytes {
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
    use super::{
        FILE_FRAME_BYTES, FileDownloadId, FileRequest, FileUploadId, MAX_FILE_BYTES,
        live_file_size_is_admitted,
    };
    use crate::operation::{GuestPath, GuestRoot};
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

    #[test]
    fn a_live_list_carries_only_validated_paths_and_bounded_page_inputs() {
        let root = GuestRoot::workspace();
        let request = FileRequest::List {
            path: GuestPath::parse(&root, "/workspace/src").expect("path"),
            recursive: true,
            limit: 1_000,
            after: Some(
                GuestPath::parse(&root, "/workspace/src/lib.rs").expect("continuation path"),
            ),
        };
        let encoded = serde_json::to_vec(&request).expect("list request encodes");
        assert!(encoded.len() < 8_192);
        assert_eq!(
            serde_json::from_slice::<FileRequest>(&encoded).expect("strict decode"),
            request
        );
    }

    #[test]
    fn the_live_file_ceiling_is_inclusive_at_exactly_five_gibibytes() {
        assert_eq!(MAX_FILE_BYTES, 5_368_709_120);
        assert!(live_file_size_is_admitted(MAX_FILE_BYTES));
        assert!(!live_file_size_is_admitted(MAX_FILE_BYTES + 1));
    }
}
