//! Generation-local, binary-safe workspace file transfers.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aex_hands_protocol::files::{
    FILE_FRAME_BYTES, FILE_TRANSFER_PART_BYTES, FileDownloadChunk, FileDownloadId,
    FileDownloadState, FileFailureCode, FilePartReceipt, FileRequest, FileResponse, FileUploadId,
    FileUploadState, LiveFileEntry, LiveFileEntryKind, LiveFileListing, MAX_FILE_LIST_ENTRIES,
    live_file_size_is_admitted,
};
use aex_hands_protocol::operation::{FileMode, GuestPath, GuestRoot};
use aex_hands_tools::{EntryKind, FsError, GuestFs, ListEntry};
use aex_wire::ids::ContentHash;
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

/// Maximum directory entries inspected by one live-list request, including
/// entries discarded before its cursor.
const MAX_FILE_LIST_VISITS: usize = 16_384;
const MAX_PARTS: u32 = 10_000;

/// Exact guest file transport state.
#[derive(Debug)]
pub struct FileService {
    root: GuestRoot,
    workspace: PathBuf,
    uploads: PathBuf,
    downloads: Mutex<BTreeMap<FileDownloadId, OpenDownload>>,
    aborted_downloads: Mutex<BTreeSet<FileDownloadId>>,
}

#[derive(Debug)]
struct OpenDownload {
    file: Option<File>,
    path: GuestPath,
    state: FileDownloadState,
    closed: Option<(ContentHash, ContentHash)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UploadManifest {
    state: FileUploadState,
    mode: FileMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PartManifest {
    receipt: FilePartReceipt,
    chunks: Vec<ChunkReceipt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ChunkReceipt {
    offset: u32,
    size_bytes: u32,
    sha256: ContentHash,
}

impl FileService {
    /// Opens the transfer store beside the operation journal.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when resumable state cannot be created.
    pub fn open(
        root: GuestRoot,
        workspace: impl Into<PathBuf>,
        journal_root: impl AsRef<Path>,
    ) -> Result<Self, std::io::Error> {
        let uploads = journal_root.as_ref().join("file-uploads");
        std::fs::create_dir_all(&uploads)?;
        Ok(Self {
            root,
            workspace: workspace.into(),
            uploads,
            downloads: Mutex::new(BTreeMap::new()),
            aborted_downloads: Mutex::new(BTreeSet::new()),
        })
    }

    /// Executes one already generation- and fence-validated request.
    #[must_use]
    pub fn answer(&self, request: FileRequest) -> FileResponse {
        self.try_answer(request)
            .unwrap_or_else(|code| FileResponse::Rejected { code })
    }

    fn try_answer(&self, request: FileRequest) -> Result<FileResponse, FileFailureCode> {
        match request {
            FileRequest::List {
                path,
                recursive,
                limit,
                after,
            } => self.list(&path, recursive, limit, after.as_ref()),
            FileRequest::Stat { path } => self.stat(&path),
            FileRequest::UploadOpen {
                upload,
                path,
                size_bytes,
                sha256,
                mode,
            } => self.upload_open(upload, path, size_bytes, sha256, mode),
            FileRequest::UploadStatus { upload } => self.upload_status(upload),
            FileRequest::UploadPartOpen {
                upload,
                part_number,
                offset,
                size_bytes,
                sha256,
            } => self.upload_part_open(upload, part_number, offset, size_bytes, sha256),
            FileRequest::UploadPartChunk {
                upload,
                part_number,
                chunk_offset,
                sha256,
                bytes,
            } => self.upload_part_chunk(upload, part_number, chunk_offset, sha256, &bytes),
            FileRequest::UploadPartComplete {
                upload,
                part_number,
            } => self.upload_part_complete(upload, part_number),
            FileRequest::UploadComplete { upload } => self.upload_complete(upload),
            FileRequest::UploadAbort { upload } => self.upload_abort(upload),
            FileRequest::DownloadOpen {
                download,
                path,
                range,
            } => self.download_open(download, &path, range),
            FileRequest::DownloadChunk {
                download,
                offset,
                max_bytes,
            } => self.download_chunk(download, offset, max_bytes),
            FileRequest::DownloadClose { download } => self.download_close(download),
            FileRequest::DownloadAbort { download } => self.download_abort(download),
        }
    }

    fn list(
        &self,
        path: &GuestPath,
        recursive: bool,
        limit: u32,
        after: Option<&GuestPath>,
    ) -> Result<FileResponse, FileFailureCode> {
        self.list_with_visit_ceiling(path, recursive, limit, after, MAX_FILE_LIST_VISITS)
    }

    fn list_with_visit_ceiling(
        &self,
        path: &GuestPath,
        recursive: bool,
        limit: u32,
        after: Option<&GuestPath>,
        visit_ceiling: usize,
    ) -> Result<FileResponse, FileFailureCode> {
        if limit == 0 || limit > MAX_FILE_LIST_ENTRIES {
            return Err(FileFailureCode::InvalidRequest);
        }
        let child_prefix = format!("{}/", path.as_str().trim_end_matches('/'));
        if after.is_some_and(|after| !after.as_str().starts_with(&child_prefix)) {
            return Err(FileFailureCode::InvalidRequest);
        }
        let fs = crate::host::HostFs::new(self.root.clone(), self.workspace.clone());
        let root_meta = fs.lstat(path).map_err(|error| map_fs(&error))?;
        if root_meta.kind != EntryKind::Directory {
            return Err(FileFailureCode::InvalidPath);
        }
        let page_size = usize::try_from(limit).map_err(|_| FileFailureCode::InvalidRequest)?;
        let mut visited = 0usize;
        let mut pending = Vec::new();
        push_children(
            &fs,
            &self.root,
            path,
            visit_ceiling,
            &mut visited,
            &mut pending,
        )?;
        let mut page = Vec::with_capacity(page_size.saturating_add(1));
        while let Some(entry) = pending.pop() {
            let entry_path = GuestPath::parse(&self.root, &entry.path)
                .map_err(|_| FileFailureCode::LimitExceeded)?;
            let before_or_at_cursor =
                after.is_some_and(|after| entry_path.as_str() <= after.as_str());
            let cursor_at_or_inside = after.is_some_and(|after| {
                after == &entry_path
                    || after
                        .as_str()
                        .starts_with(&format!("{}/", entry_path.as_str()))
            });
            let descend = recursive
                && entry.kind == EntryKind::Directory
                && !entry_path.as_str().ends_with("/.git")
                && (!before_or_at_cursor || cursor_at_or_inside);
            if !before_or_at_cursor {
                page.push(entry);
                if page.len() > page_size {
                    break;
                }
            }
            if descend {
                push_children(
                    &fs,
                    &self.root,
                    &entry_path,
                    visit_ceiling,
                    &mut visited,
                    &mut pending,
                )?;
            }
        }
        let truncated = page.len() > page_size;
        page.truncate(page_size);
        let next_after = truncated
            .then(|| page.last().map(|entry| entry.path.as_str()))
            .flatten()
            .map(|path| GuestPath::parse(&self.root, path))
            .transpose()
            .map_err(|_| FileFailureCode::Unavailable)?;
        let entries = page
            .into_iter()
            .map(|entry| live_entry(&self.root, entry))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(FileResponse::Listing {
            listing: LiveFileListing {
                entries,
                next_after,
            },
        })
    }

    fn stat(&self, path: &GuestPath) -> Result<FileResponse, FileFailureCode> {
        let fs = crate::host::HostFs::new(self.root.clone(), self.workspace.clone());
        let entry =
            aex_hands_tools::filesystem::stat_path(&fs, path).map_err(|error| map_fs(&error))?;
        Ok(FileResponse::Entry {
            entry: live_entry(&self.root, entry)?,
        })
    }

    fn upload_open(
        &self,
        upload: FileUploadId,
        path: GuestPath,
        size_bytes: u64,
        sha256: ContentHash,
        mode: FileMode,
    ) -> Result<FileResponse, FileFailureCode> {
        if !live_file_size_is_admitted(size_bytes) || path.as_str() == self.root.0 {
            return Err(FileFailureCode::LimitExceeded);
        }
        if self.aborted_upload_path(upload).exists() {
            return Err(FileFailureCode::Conflict);
        }
        self.validate_workspace_path(&path, true)?;
        let directory = self.upload_directory(upload);
        if directory.exists() {
            let manifest = self.read_manifest(upload)?;
            if manifest.state.path != path
                || manifest.state.size_bytes != size_bytes
                || manifest.state.sha256 != sha256
                || manifest.mode != mode
            {
                return Err(FileFailureCode::Conflict);
            }
            return Ok(FileResponse::Upload {
                state: manifest.state,
            });
        }
        std::fs::create_dir(&directory).map_err(|error| map_io(&error))?;
        let manifest = UploadManifest {
            state: FileUploadState {
                upload,
                path,
                size_bytes,
                sha256,
                parts: Vec::new(),
                complete: false,
            },
            mode,
        };
        self.write_manifest(&manifest)?;
        Ok(FileResponse::Upload {
            state: manifest.state,
        })
    }

    fn upload_status(&self, upload: FileUploadId) -> Result<FileResponse, FileFailureCode> {
        Ok(FileResponse::Upload {
            state: self.read_manifest(upload)?.state,
        })
    }

    fn upload_part_open(
        &self,
        upload: FileUploadId,
        part_number: u32,
        offset: u64,
        size_bytes: u32,
        sha256: ContentHash,
    ) -> Result<FileResponse, FileFailureCode> {
        let manifest = self.read_manifest(upload)?;
        if manifest.state.complete {
            return Err(FileFailureCode::Conflict);
        }
        let expected_offset = u64::from(part_number.saturating_sub(1))
            .checked_mul(u64::from(FILE_TRANSFER_PART_BYTES))
            .ok_or(FileFailureCode::InvalidRequest)?;
        if part_number == 0
            || part_number > MAX_PARTS
            || offset != expected_offset
            || offset >= manifest.state.size_bytes
        {
            return Err(FileFailureCode::InvalidRequest);
        }
        let expected_size =
            (manifest.state.size_bytes - offset).min(u64::from(FILE_TRANSFER_PART_BYTES));
        if u64::from(size_bytes) != expected_size {
            return Err(FileFailureCode::InvalidRequest);
        }
        let receipt = FilePartReceipt {
            part_number,
            offset,
            size_bytes,
            sha256,
        };
        if let Some(existing) = manifest
            .state
            .parts
            .iter()
            .find(|part| part.part_number == part_number)
        {
            if existing != &receipt {
                return Err(FileFailureCode::Conflict);
            }
            return Ok(FileResponse::Upload {
                state: manifest.state,
            });
        }
        let part_manifest = PartManifest {
            receipt,
            chunks: Vec::new(),
        };
        match self.read_part_manifest(upload, part_number) {
            Ok(existing) if existing == part_manifest => {}
            Ok(_) => return Err(FileFailureCode::Conflict),
            Err(FileFailureCode::NotFound) => self.write_part_manifest(upload, &part_manifest)?,
            Err(error) => return Err(error),
        }
        Ok(FileResponse::Upload {
            state: manifest.state,
        })
    }

    fn upload_part_chunk(
        &self,
        upload: FileUploadId,
        part_number: u32,
        chunk_offset: u32,
        sha256: ContentHash,
        bytes: &[u8],
    ) -> Result<FileResponse, FileFailureCode> {
        let upload_manifest = self.read_manifest(upload)?;
        if upload_manifest.state.complete
            || upload_manifest
                .state
                .parts
                .iter()
                .any(|part| part.part_number == part_number)
        {
            return Err(FileFailureCode::Conflict);
        }
        let mut part = self.read_part_manifest(upload, part_number)?;
        let chunk_size = u32::try_from(bytes.len()).map_err(|_| FileFailureCode::InvalidRequest)?;
        let chunk_end = chunk_offset
            .checked_add(chunk_size)
            .ok_or(FileFailureCode::InvalidRequest)?;
        let expected_size = part
            .receipt
            .size_bytes
            .saturating_sub(chunk_offset)
            .min(FILE_FRAME_BYTES);
        if bytes.is_empty()
            || bytes.len() > FILE_FRAME_BYTES as usize
            || chunk_offset >= part.receipt.size_bytes
            || !chunk_offset.is_multiple_of(FILE_FRAME_BYTES)
            || chunk_size != expected_size
            || chunk_end > part.receipt.size_bytes
            || ContentHash::of(bytes) != sha256
        {
            return Err(FileFailureCode::InvalidRequest);
        }
        let receipt = ChunkReceipt {
            offset: chunk_offset,
            size_bytes: chunk_size,
            sha256,
        };
        if let Some(existing) = part
            .chunks
            .iter()
            .find(|chunk| chunk.offset == chunk_offset)
        {
            if existing != &receipt
                || std::fs::read(self.chunk_path(upload, part_number, chunk_offset))
                    .map_err(|error| map_io(&error))?
                    != bytes
            {
                return Err(FileFailureCode::Conflict);
            }
            return Ok(FileResponse::Upload {
                state: upload_manifest.state,
            });
        }
        write_atomic(&self.chunk_path(upload, part_number, chunk_offset), bytes)?;
        part.chunks.push(receipt);
        part.chunks.sort_by_key(|chunk| chunk.offset);
        self.write_part_manifest(upload, &part)?;
        Ok(FileResponse::Upload {
            state: upload_manifest.state,
        })
    }

    fn upload_part_complete(
        &self,
        upload: FileUploadId,
        part_number: u32,
    ) -> Result<FileResponse, FileFailureCode> {
        let mut upload_manifest = self.read_manifest(upload)?;
        if upload_manifest.state.complete {
            return Err(FileFailureCode::Conflict);
        }
        if upload_manifest
            .state
            .parts
            .iter()
            .any(|part| part.part_number == part_number)
        {
            return Ok(FileResponse::Upload {
                state: upload_manifest.state,
            });
        }
        let part = self.read_part_manifest(upload, part_number)?;
        let expected_chunks = part.receipt.size_bytes.div_ceil(FILE_FRAME_BYTES);
        if u32::try_from(part.chunks.len()).map_err(|_| FileFailureCode::InvalidRequest)?
            != expected_chunks
        {
            return Err(FileFailureCode::InvalidRequest);
        }
        for (index, chunk) in part.chunks.iter().enumerate() {
            let offset = u32::try_from(index)
                .map_err(|_| FileFailureCode::InvalidRequest)?
                .checked_mul(FILE_FRAME_BYTES)
                .ok_or(FileFailureCode::InvalidRequest)?;
            if chunk.offset != offset {
                return Err(FileFailureCode::InvalidRequest);
            }
        }

        let target = self.part_path(upload, part_number);
        let temporary = target.with_extension("aex-part-new");
        let mut output = open_new_truncated(&temporary).map_err(|error| map_io(&error))?;
        let mut hash = sha2::Sha256::new();
        let mut assembled = 0u64;
        for chunk in &part.chunks {
            let bytes = std::fs::read(self.chunk_path(upload, part_number, chunk.offset))
                .map_err(|error| map_io(&error))?;
            if u32::try_from(bytes.len()) != Ok(chunk.size_bytes)
                || ContentHash::of(&bytes) != chunk.sha256
            {
                let _ = std::fs::remove_file(&temporary);
                return Err(FileFailureCode::Conflict);
            }
            output.write_all(&bytes).map_err(|error| map_io(&error))?;
            hash.update(&bytes);
            assembled = assembled.saturating_add(bytes.len() as u64);
        }
        if assembled != u64::from(part.receipt.size_bytes)
            || ContentHash::from_bytes(hash.finalize().into()) != part.receipt.sha256
        {
            let _ = std::fs::remove_file(&temporary);
            return Err(FileFailureCode::InvalidRequest);
        }
        output.sync_all().map_err(|error| map_io(&error))?;
        drop(output);
        std::fs::rename(&temporary, &target).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            map_io(&error)
        })?;
        #[cfg(unix)]
        sync_directory(self.upload_directory(upload).as_path())?;
        #[cfg(not(unix))]
        sync_directory(self.upload_directory(upload).as_path());
        upload_manifest.state.parts.push(part.receipt);
        upload_manifest
            .state
            .parts
            .sort_by_key(|receipt| receipt.part_number);
        self.write_manifest(&upload_manifest)?;
        let _ = std::fs::remove_file(self.part_manifest_path(upload, part_number));
        for chunk in &part.chunks {
            let _ = std::fs::remove_file(self.chunk_path(upload, part_number, chunk.offset));
        }
        Ok(FileResponse::Upload {
            state: upload_manifest.state,
        })
    }

    fn upload_complete(&self, upload: FileUploadId) -> Result<FileResponse, FileFailureCode> {
        let mut manifest = self.read_manifest(upload)?;
        if manifest.state.complete {
            return Ok(FileResponse::UploadComplete {
                state: manifest.state,
            });
        }
        let expected_parts = if manifest.state.size_bytes == 0 {
            0
        } else {
            manifest
                .state
                .size_bytes
                .div_ceil(u64::from(FILE_TRANSFER_PART_BYTES))
        };
        if manifest.state.parts.len() as u64 != expected_parts {
            return Err(FileFailureCode::InvalidRequest);
        }
        for (index, part) in manifest.state.parts.iter().enumerate() {
            let number = u32::try_from(index + 1).map_err(|_| FileFailureCode::InvalidRequest)?;
            if part.part_number != number
                || part.offset != u64::from(number - 1) * u64::from(FILE_TRANSFER_PART_BYTES)
            {
                return Err(FileFailureCode::InvalidRequest);
            }
        }

        self.validate_workspace_path(&manifest.state.path, true)?;
        let target = self.host_path(&manifest.state.path);
        let parent = target.parent().ok_or(FileFailureCode::InvalidPath)?;
        let temporary = parent.join(format!(".aex-upload-{upload}"));
        let mut output = open_new_truncated(&temporary).map_err(|error| map_io(&error))?;
        let mut whole = sha2::Sha256::new();
        let mut assembled = 0u64;
        for receipt in &manifest.state.parts {
            let mut part = File::open(self.part_path(upload, receipt.part_number))
                .map_err(|error| map_io(&error))?;
            let mut buffer = vec![0u8; 64 * 1024];
            let mut part_hash = sha2::Sha256::new();
            let mut part_bytes = 0u64;
            loop {
                let read = part.read(&mut buffer).map_err(|error| map_io(&error))?;
                if read == 0 {
                    break;
                }
                output
                    .write_all(&buffer[..read])
                    .map_err(|error| map_io(&error))?;
                whole.update(&buffer[..read]);
                part_hash.update(&buffer[..read]);
                part_bytes = part_bytes.saturating_add(read as u64);
            }
            if part_bytes != u64::from(receipt.size_bytes)
                || ContentHash::from_bytes(part_hash.finalize().into()) != receipt.sha256
            {
                let _ = std::fs::remove_file(&temporary);
                return Err(FileFailureCode::Conflict);
            }
            assembled = assembled.saturating_add(part_bytes);
        }
        if assembled != manifest.state.size_bytes
            || ContentHash::from_bytes(whole.finalize().into()) != manifest.state.sha256
        {
            let _ = std::fs::remove_file(&temporary);
            return Err(FileFailureCode::InvalidRequest);
        }
        #[cfg(unix)]
        set_mode(&output, manifest.mode).map_err(|error| map_io(&error))?;
        #[cfg(not(unix))]
        set_mode(&output, manifest.mode);
        output.sync_all().map_err(|error| map_io(&error))?;
        drop(output);
        self.validate_workspace_path(&manifest.state.path, true)?;
        std::fs::rename(&temporary, &target).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            map_io(&error)
        })?;
        #[cfg(unix)]
        sync_directory(parent)?;
        #[cfg(not(unix))]
        sync_directory(parent);
        manifest.state.complete = true;
        self.write_manifest(&manifest)?;
        for receipt in &manifest.state.parts {
            let _ = std::fs::remove_file(self.part_path(upload, receipt.part_number));
        }
        Ok(FileResponse::UploadComplete {
            state: manifest.state,
        })
    }

    fn upload_abort(&self, upload: FileUploadId) -> Result<FileResponse, FileFailureCode> {
        if self.aborted_upload_path(upload).exists() {
            let directory = self.upload_directory(upload);
            if directory.exists() {
                std::fs::remove_dir_all(directory).map_err(|error| map_io(&error))?;
            }
            return Ok(FileResponse::UploadAborted { upload });
        }
        if self.read_manifest(upload)?.state.complete {
            return Err(FileFailureCode::Conflict);
        }
        write_atomic(&self.aborted_upload_path(upload), b"aborted")?;
        std::fs::remove_dir_all(self.upload_directory(upload)).map_err(|error| map_io(&error))?;
        Ok(FileResponse::UploadAborted { upload })
    }

    fn download_open(
        &self,
        download: FileDownloadId,
        path: &GuestPath,
        range: Option<aex_hands_protocol::operation::ByteRangeRequest>,
    ) -> Result<FileResponse, FileFailureCode> {
        self.validate_workspace_path(path, false)?;
        let mut file = open_read_nofollow(&self.host_path(path)).map_err(|error| map_io(&error))?;
        let metadata = file.metadata().map_err(|error| map_io(&error))?;
        if !metadata.is_file() {
            return Err(FileFailureCode::InvalidPath);
        }
        let file_size_bytes = metadata.len();
        if !live_file_size_is_admitted(file_size_bytes) {
            return Err(FileFailureCode::LimitExceeded);
        }
        let (sha256, parts) = hash_file_parts(&mut file)?;
        let version = file_version(&metadata, sha256);
        let (start, length_bytes) = match range {
            None => (0, file_size_bytes),
            Some(range) => {
                let start = u64::try_from(range.start.get())
                    .map_err(|_| FileFailureCode::InvalidRequest)?;
                let end = u64::try_from(range.end_inclusive.get())
                    .map_err(|_| FileFailureCode::InvalidRequest)?;
                if start > end || start >= file_size_bytes || end >= file_size_bytes {
                    return Err(FileFailureCode::InvalidRequest);
                }
                (start, end - start + 1)
            }
        };
        let state = FileDownloadState {
            download,
            file_size_bytes,
            start,
            length_bytes,
            sha256,
            parts,
            version,
        };
        let mut open = self
            .downloads
            .lock()
            .map_err(|_| FileFailureCode::Unavailable)?;
        if let Some(existing) = open.get(&download) {
            if existing.state != state || existing.file.is_none() {
                return Err(FileFailureCode::Conflict);
            }
        } else {
            open.insert(
                download,
                OpenDownload {
                    file: Some(file),
                    path: path.clone(),
                    state: state.clone(),
                    closed: None,
                },
            );
        }
        Ok(FileResponse::DownloadOpened { state })
    }

    fn download_chunk(
        &self,
        download: FileDownloadId,
        offset: u64,
        max_bytes: u32,
    ) -> Result<FileResponse, FileFailureCode> {
        if max_bytes == 0 || max_bytes > FILE_FRAME_BYTES {
            return Err(FileFailureCode::InvalidRequest);
        }
        let mut open = self
            .downloads
            .lock()
            .map_err(|_| FileFailureCode::Unavailable)?;
        let opened = open.get_mut(&download).ok_or(FileFailureCode::NotFound)?;
        let file = opened.file.as_mut().ok_or(FileFailureCode::Conflict)?;
        let end = opened
            .state
            .start
            .checked_add(opened.state.length_bytes)
            .ok_or(FileFailureCode::InvalidRequest)?;
        if offset < opened.state.start || offset > end {
            return Err(FileFailureCode::InvalidRequest);
        }
        let wanted = (end - offset).min(u64::from(max_bytes));
        let mut bytes =
            vec![0u8; usize::try_from(wanted).map_err(|_| FileFailureCode::InvalidRequest)?];
        file.seek(std::io::SeekFrom::Start(offset))
            .map_err(|error| map_io(&error))?;
        file.read_exact(&mut bytes)
            .map_err(|error| map_io(&error))?;
        let chunk = FileDownloadChunk {
            download,
            offset,
            sha256: ContentHash::of(&bytes),
            last: offset.saturating_add(bytes.len() as u64) == end,
            bytes,
        };
        Ok(FileResponse::DownloadChunk { chunk })
    }

    fn download_close(&self, download: FileDownloadId) -> Result<FileResponse, FileFailureCode> {
        let mut open = self
            .downloads
            .lock()
            .map_err(|_| FileFailureCode::Unavailable)?;
        let opened = open.get_mut(&download).ok_or(FileFailureCode::NotFound)?;
        if let Some((sha256, version)) = opened.closed {
            return Ok(FileResponse::DownloadClosed {
                download,
                sha256,
                version,
            });
        }
        let file = opened.file.as_mut().ok_or(FileFailureCode::Unavailable)?;
        let descriptor_metadata = file.metadata().map_err(|error| map_io(&error))?;
        let descriptor_sha = hash_file(file)?;
        if descriptor_sha != opened.state.sha256
            || file_version(&descriptor_metadata, descriptor_sha) != opened.state.version
        {
            return Err(FileFailureCode::Conflict);
        }
        self.validate_workspace_path(&opened.path, false)?;
        let mut current =
            open_read_nofollow(&self.host_path(&opened.path)).map_err(|error| map_io(&error))?;
        let current_metadata = current.metadata().map_err(|error| map_io(&error))?;
        let current_sha = hash_file(&mut current)?;
        if current_sha != opened.state.sha256
            || file_version(&current_metadata, current_sha) != opened.state.version
        {
            return Err(FileFailureCode::Conflict);
        }
        opened.closed = Some((descriptor_sha, opened.state.version));
        opened.file = None;
        Ok(FileResponse::DownloadClosed {
            download,
            sha256: descriptor_sha,
            version: opened.state.version,
        })
    }

    fn download_abort(&self, download: FileDownloadId) -> Result<FileResponse, FileFailureCode> {
        let mut aborted = self
            .aborted_downloads
            .lock()
            .map_err(|_| FileFailureCode::Unavailable)?;
        if aborted.contains(&download) {
            return Ok(FileResponse::DownloadAborted { download });
        }
        self.downloads
            .lock()
            .map_err(|_| FileFailureCode::Unavailable)?
            .remove(&download)
            .ok_or(FileFailureCode::NotFound)?;
        aborted.insert(download);
        Ok(FileResponse::DownloadAborted { download })
    }

    fn validate_workspace_path(
        &self,
        path: &GuestPath,
        final_may_be_missing: bool,
    ) -> Result<(), FileFailureCode> {
        let relative = path
            .as_str()
            .strip_prefix(self.root.0.trim_end_matches('/'))
            .ok_or(FileFailureCode::InvalidPath)?
            .trim_start_matches('/');
        if relative.is_empty() {
            return Err(FileFailureCode::InvalidPath);
        }
        let components: Vec<&str> = relative.split('/').collect();
        let mut current = self.workspace.clone();
        for (index, component) in components.iter().enumerate() {
            if component.is_empty() {
                return Err(FileFailureCode::InvalidPath);
            }
            current.push(component);
            match std::fs::symlink_metadata(&current) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink()
                        || (index + 1 < components.len() && !metadata.is_dir())
                        || (index + 1 == components.len() && metadata.is_dir())
                    {
                        return Err(FileFailureCode::InvalidPath);
                    }
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && final_may_be_missing
                        && index + 1 == components.len() => {}
                Err(error) => return Err(map_io(&error)),
            }
        }
        Ok(())
    }

    fn host_path(&self, path: &GuestPath) -> PathBuf {
        let relative = path
            .as_str()
            .strip_prefix(self.root.0.trim_end_matches('/'))
            .unwrap_or(path.as_str())
            .trim_start_matches('/');
        self.workspace.join(relative)
    }

    fn upload_directory(&self, upload: FileUploadId) -> PathBuf {
        self.uploads.join(upload.to_string())
    }
    fn aborted_upload_path(&self, upload: FileUploadId) -> PathBuf {
        self.uploads.join(format!("aborted-{upload}"))
    }
    fn manifest_path(&self, upload: FileUploadId) -> PathBuf {
        self.upload_directory(upload).join("manifest.json")
    }
    fn part_path(&self, upload: FileUploadId, number: u32) -> PathBuf {
        self.upload_directory(upload)
            .join(format!("part-{number:05}.body"))
    }
    fn part_manifest_path(&self, upload: FileUploadId, number: u32) -> PathBuf {
        self.upload_directory(upload)
            .join(format!("part-{number:05}.json"))
    }
    fn chunk_path(&self, upload: FileUploadId, number: u32, offset: u32) -> PathBuf {
        self.upload_directory(upload)
            .join(format!("part-{number:05}-chunk-{offset:010}.body"))
    }
    fn read_manifest(&self, upload: FileUploadId) -> Result<UploadManifest, FileFailureCode> {
        let bytes = std::fs::read(self.manifest_path(upload)).map_err(|error| map_io(&error))?;
        serde_json::from_slice(&bytes).map_err(|_| FileFailureCode::Conflict)
    }
    fn write_manifest(&self, manifest: &UploadManifest) -> Result<(), FileFailureCode> {
        let bytes = serde_json::to_vec(manifest).map_err(|_| FileFailureCode::Unavailable)?;
        write_atomic(&self.manifest_path(manifest.state.upload), &bytes)
    }
    fn read_part_manifest(
        &self,
        upload: FileUploadId,
        number: u32,
    ) -> Result<PartManifest, FileFailureCode> {
        let bytes = std::fs::read(self.part_manifest_path(upload, number))
            .map_err(|error| map_io(&error))?;
        serde_json::from_slice(&bytes).map_err(|_| FileFailureCode::Conflict)
    }
    fn write_part_manifest(
        &self,
        upload: FileUploadId,
        manifest: &PartManifest,
    ) -> Result<(), FileFailureCode> {
        let bytes = serde_json::to_vec(manifest).map_err(|_| FileFailureCode::Unavailable)?;
        write_atomic(
            &self.part_manifest_path(upload, manifest.receipt.part_number),
            &bytes,
        )
    }
}

fn push_children(
    fs: &crate::host::HostFs,
    root: &GuestRoot,
    directory: &GuestPath,
    visit_ceiling: usize,
    visited: &mut usize,
    pending: &mut Vec<ListEntry>,
) -> Result<(), FileFailureCode> {
    let remaining = visit_ceiling
        .checked_sub(*visited)
        .ok_or(FileFailureCode::LimitExceeded)?;
    let children = fs
        .read_dir_bounded(directory, remaining)
        .map_err(|error| map_fs(&error))?
        .ok_or(FileFailureCode::LimitExceeded)?;
    *visited = visited
        .checked_add(children.len())
        .ok_or(FileFailureCode::LimitExceeded)?;
    for child in children.into_iter().rev() {
        let path = format!(
            "{}/{}",
            directory.as_str().trim_end_matches('/'),
            child.name
        );
        GuestPath::parse(root, &path).map_err(|_| FileFailureCode::LimitExceeded)?;
        pending.push(ListEntry {
            path,
            kind: child.meta.kind,
            size: child.meta.size,
            mtime_ms: child.meta.mtime_ms,
            mode: child.meta.mode,
            target: child.meta.target,
        });
    }
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), FileFailureCode> {
    let temporary = path.with_extension("aex-new");
    let mut file = open_new_truncated(&temporary).map_err(|error| map_io(&error))?;
    file.write_all(bytes).map_err(|error| map_io(&error))?;
    file.sync_all().map_err(|error| map_io(&error))?;
    drop(file);
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        map_io(&error)
    })?;
    if let Some(parent) = path.parent() {
        #[cfg(unix)]
        sync_directory(parent)?;
        #[cfg(not(unix))]
        sync_directory(parent);
    }
    Ok(())
}

fn open_new_truncated(path: &Path) -> Result<File, std::io::Error> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
}

fn open_read_nofollow(path: &Path) -> Result<File, std::io::Error> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        let nofollow = i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        options.custom_flags(nofollow);
    }
    options.open(path)
}

fn hash_file(file: &mut File) -> Result<ContentHash, FileFailureCode> {
    file.seek(std::io::SeekFrom::Start(0))
        .map_err(|error| map_io(&error))?;
    let mut hash = sha2::Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| map_io(&error))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    file.seek(std::io::SeekFrom::Start(0))
        .map_err(|error| map_io(&error))?;
    Ok(ContentHash::from_bytes(hash.finalize().into()))
}

fn hash_file_parts(
    file: &mut File,
) -> Result<(ContentHash, Vec<FilePartReceipt>), FileFailureCode> {
    file.seek(std::io::SeekFrom::Start(0))
        .map_err(|error| map_io(&error))?;
    let mut whole = sha2::Sha256::new();
    let mut part = sha2::Sha256::new();
    let mut parts = Vec::new();
    let mut part_bytes = 0u32;
    let mut offset = 0u64;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| map_io(&error))?;
        if read == 0 {
            break;
        }
        let mut consumed = 0usize;
        while consumed < read {
            let available = FILE_TRANSFER_PART_BYTES - part_bytes;
            let take = usize::try_from(available)
                .map_err(|_| FileFailureCode::InvalidRequest)?
                .min(read - consumed);
            whole.update(&buffer[consumed..consumed + take]);
            part.update(&buffer[consumed..consumed + take]);
            let take_bytes = u32::try_from(take).map_err(|_| FileFailureCode::InvalidRequest)?;
            part_bytes = part_bytes.saturating_add(take_bytes);
            consumed += take;
            if part_bytes == FILE_TRANSFER_PART_BYTES {
                let number =
                    u32::try_from(parts.len() + 1).map_err(|_| FileFailureCode::LimitExceeded)?;
                let completed = std::mem::replace(&mut part, sha2::Sha256::new());
                parts.push(FilePartReceipt {
                    part_number: number,
                    offset,
                    size_bytes: part_bytes,
                    sha256: ContentHash::from_bytes(completed.finalize().into()),
                });
                offset = offset.saturating_add(u64::from(part_bytes));
                part_bytes = 0;
            }
        }
    }
    if part_bytes != 0 {
        let number = u32::try_from(parts.len() + 1).map_err(|_| FileFailureCode::LimitExceeded)?;
        parts.push(FilePartReceipt {
            part_number: number,
            offset,
            size_bytes: part_bytes,
            sha256: ContentHash::from_bytes(part.finalize().into()),
        });
    }
    file.seek(std::io::SeekFrom::Start(0))
        .map_err(|error| map_io(&error))?;
    Ok((ContentHash::from_bytes(whole.finalize().into()), parts))
}

fn file_version(metadata: &std::fs::Metadata, sha256: ContentHash) -> ContentHash {
    let mut evidence = Vec::with_capacity(128);
    evidence.extend_from_slice(&metadata.len().to_be_bytes());
    evidence.extend_from_slice(sha256.as_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        for value in [metadata.dev(), metadata.ino()] {
            evidence.extend_from_slice(&value.to_be_bytes());
        }
        for value in [
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        ] {
            evidence.extend_from_slice(&value.to_be_bytes());
        }
    }
    #[cfg(not(unix))]
    for instant in [metadata.created(), metadata.modified()] {
        let nanos = instant
            .ok()
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |value| value.as_nanos());
        evidence.extend_from_slice(&nanos.to_be_bytes());
    }
    ContentHash::of(&evidence)
}

#[cfg(unix)]
fn set_mode(file: &File, mode: FileMode) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt as _;
    let bits = match mode {
        FileMode::ReadWrite => 0o644,
        FileMode::Executable => 0o755,
    };
    file.set_permissions(std::fs::Permissions::from_mode(bits))
}
#[cfg(not(unix))]
fn set_mode(_file: &File, _mode: FileMode) {}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), FileFailureCode> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| map_io(&error))?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) {}

fn map_io(error: &std::io::Error) -> FileFailureCode {
    match error.kind() {
        std::io::ErrorKind::NotFound => FileFailureCode::NotFound,
        std::io::ErrorKind::StorageFull => FileFailureCode::LimitExceeded,
        std::io::ErrorKind::InvalidInput => FileFailureCode::InvalidRequest,
        _ => FileFailureCode::Unavailable,
    }
}

fn map_fs(error: &FsError) -> FileFailureCode {
    match error {
        FsError::NotFound { .. } => FileFailureCode::NotFound,
        FsError::IsADirectory { .. } | FsError::NotADirectory { .. } => {
            FileFailureCode::InvalidPath
        }
        FsError::DiskFull => FileFailureCode::LimitExceeded,
        FsError::PermissionDenied { .. } | FsError::Other { .. } => FileFailureCode::Unavailable,
    }
}

fn live_entry(root: &GuestRoot, entry: ListEntry) -> Result<LiveFileEntry, FileFailureCode> {
    let path = GuestPath::parse(root, &entry.path).map_err(|_| FileFailureCode::Unavailable)?;
    let kind = match entry.kind {
        EntryKind::File => LiveFileEntryKind::File,
        EntryKind::Directory => LiveFileEntryKind::Directory,
        EntryKind::Symlink => LiveFileEntryKind::Symlink,
        EntryKind::Other => return Err(FileFailureCode::InvalidPath),
    };
    let target = match (kind, entry.target) {
        (LiveFileEntryKind::Symlink, Some(target))
            if !target.is_empty() && target.len() <= GuestPath::MAX_BYTES =>
        {
            Some(target)
        }
        (LiveFileEntryKind::Symlink, Some(_)) => return Err(FileFailureCode::LimitExceeded),
        (LiveFileEntryKind::Symlink, None) | (_, Some(_)) => {
            return Err(FileFailureCode::Unavailable);
        }
        (_, None) => None,
    };
    Ok(LiveFileEntry {
        path,
        kind,
        size_bytes: entry.size,
        mtime_ms: entry.mtime_ms,
        mode: entry.mode & 0o777,
        target,
    })
}

#[cfg(test)]
mod tests {
    use super::FileService;
    use aex_hands_protocol::files::{
        FILE_FRAME_BYTES, FILE_TRANSFER_PART_BYTES, FileDownloadId, FileRequest, FileResponse,
        FileUploadId,
    };
    use aex_hands_protocol::operation::{FileMode, GuestPath, GuestRoot};
    use aex_wire::ids::{ContentHash, Uuid7};

    fn upload(seed: u8) -> FileUploadId {
        FileUploadId(Uuid7::compose(1, [seed; 10]))
    }
    fn path(root: &GuestRoot, value: &str) -> GuestPath {
        GuestPath::parse(root, &format!("{}/{value}", root.0)).expect("contained path")
    }

    fn open_part(
        service: &FileService,
        upload: FileUploadId,
        part_number: u32,
        offset: u64,
        bytes: &[u8],
    ) {
        assert!(matches!(
            service.answer(FileRequest::UploadPartOpen {
                upload,
                part_number,
                offset,
                size_bytes: u32::try_from(bytes.len()).expect("bounded part"),
                sha256: ContentHash::of(bytes),
            }),
            FileResponse::Upload { .. }
        ));
    }

    fn write_part_chunks(
        service: &FileService,
        upload: FileUploadId,
        part_number: u32,
        bytes: &[u8],
    ) {
        for (index, frame) in bytes.chunks(FILE_FRAME_BYTES as usize).enumerate() {
            let chunk_offset = u32::try_from(index).expect("bounded frames") * FILE_FRAME_BYTES;
            assert!(matches!(
                service.answer(FileRequest::UploadPartChunk {
                    upload,
                    part_number,
                    chunk_offset,
                    sha256: ContentHash::of(frame),
                    bytes: frame.to_vec(),
                }),
                FileResponse::Upload { .. }
            ));
        }
        assert!(matches!(
            service.answer(FileRequest::UploadPartComplete {
                upload,
                part_number,
            }),
            FileResponse::Upload { .. }
        ));
    }

    #[test]
    fn multipart_upload_replays_parts_and_publishes_only_after_whole_sha_verification() {
        let directory = tempfile::tempdir().expect("temporary root");
        let workspace = directory.path().join("workspace");
        let journal = directory.path().join("journal");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let root = GuestRoot::workspace();
        let service = FileService::open(root.clone(), &workspace, &journal).expect("service");
        let mut bytes = vec![b'a'; FILE_TRANSFER_PART_BYTES as usize];
        bytes.extend_from_slice(b"tail");
        let identity = upload(1);
        let destination = path(&root, "artifact.bin");
        assert!(matches!(
            service.answer(FileRequest::UploadOpen {
                upload: identity,
                path: destination,
                size_bytes: u64::try_from(bytes.len()).expect("bounded fixture"),
                sha256: ContentHash::of(&bytes),
                mode: FileMode::ReadWrite,
            }),
            FileResponse::Upload { .. }
        ));
        assert!(!workspace.join("artifact.bin").exists());
        let first_part = &bytes[..FILE_TRANSFER_PART_BYTES as usize];
        open_part(&service, identity, 1, 0, first_part);
        let first_frame = FileRequest::UploadPartChunk {
            upload: identity,
            part_number: 1,
            chunk_offset: 0,
            sha256: ContentHash::of(&first_part[..FILE_FRAME_BYTES as usize]),
            bytes: first_part[..FILE_FRAME_BYTES as usize].to_vec(),
        };
        assert_eq!(
            service.answer(first_frame.clone()),
            service.answer(first_frame.clone())
        );
        drop(service);

        let service = FileService::open(root.clone(), &workspace, &journal).expect("resume");
        assert!(matches!(
            service.answer(first_frame),
            FileResponse::Upload { .. }
        ));
        write_part_chunks(&service, identity, 1, first_part);
        open_part(
            &service,
            identity,
            2,
            u64::from(FILE_TRANSFER_PART_BYTES),
            b"tail",
        );
        write_part_chunks(&service, identity, 2, b"tail");
        assert!(matches!(
            service.answer(FileRequest::UploadComplete { upload: identity }),
            FileResponse::UploadComplete { .. }
        ));
        assert_eq!(
            std::fs::read(workspace.join("artifact.bin")).expect("file"),
            bytes
        );
        let reopened = FileService::open(root, &workspace, &journal).expect("reopen");
        assert!(
            matches!(reopened.answer(FileRequest::UploadStatus { upload: identity }),
            FileResponse::Upload { state } if state.complete)
        );
    }

    #[test]
    fn ranged_download_is_binary_safe_and_fd_pinned() {
        let directory = tempfile::tempdir().expect("temporary root");
        let workspace = directory.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(workspace.join("binary"), [0, 255, 1, 2, 3]).expect("fixture");
        let root = GuestRoot::workspace();
        let service = FileService::open(root.clone(), &workspace, directory.path().join("journal"))
            .expect("service");
        let id = FileDownloadId(Uuid7::compose(1, [7; 10]));
        assert!(matches!(service.answer(FileRequest::DownloadOpen {
            download: id, path: path(&root, "binary"),
            range: Some(aex_hands_protocol::operation::ByteRangeRequest {
                start: aex_wire::types::DecimalU128::new(1),
                end_inclusive: aex_wire::types::DecimalU128::new(3),
            }),
        }), FileResponse::DownloadOpened { state }
            if state.start == 1
                && state.length_bytes == 3
                && state.file_size_bytes == 5
                && state.parts.len() == 1
                && state.parts[0].sha256 == ContentHash::of(&[0, 255, 1, 2, 3])));
        std::fs::rename(workspace.join("binary"), workspace.join("moved")).expect("rename");
        std::fs::write(workspace.join("binary"), b"replacement").expect("replacement");
        assert!(
            matches!(service.answer(FileRequest::DownloadChunk { download: id, offset: 1, max_bytes: 3 }),
            FileResponse::DownloadChunk { chunk }
                if chunk.bytes == [255, 1, 2] && chunk.sha256 == ContentHash::of(&[255, 1, 2]) && chunk.last)
        );
        assert!(matches!(
            service.answer(FileRequest::DownloadClose { download: id }),
            FileResponse::Rejected {
                code: aex_hands_protocol::files::FileFailureCode::Conflict
            }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn uploads_and_downloads_never_follow_workspace_symlinks() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary root");
        let workspace = directory.path().join("workspace");
        let outside = directory.path().join("outside");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&outside).expect("outside");
        std::fs::write(outside.join("secret"), b"secret").expect("fixture");
        symlink(&outside, workspace.join("escape")).expect("directory symlink");
        symlink(outside.join("secret"), workspace.join("secret-link")).expect("file symlink");

        let root = GuestRoot::workspace();
        let service = FileService::open(root.clone(), &workspace, directory.path().join("journal"))
            .expect("service");
        let rejected = |response| {
            matches!(
                response,
                FileResponse::Rejected {
                    code: aex_hands_protocol::files::FileFailureCode::InvalidPath
                }
            )
        };

        assert!(rejected(service.answer(FileRequest::UploadOpen {
            upload: upload(2),
            path: path(&root, "escape/new"),
            size_bytes: 1,
            sha256: ContentHash::of(b"x"),
            mode: FileMode::ReadWrite,
        })));
        assert!(rejected(service.answer(FileRequest::DownloadOpen {
            download: FileDownloadId(Uuid7::compose(1, [8; 10])),
            path: path(&root, "secret-link"),
            range: None,
        })));
        assert_eq!(
            std::fs::read(outside.join("secret")).expect("outside file remains"),
            b"secret"
        );
    }
}
