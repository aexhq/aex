//! Generation-local, binary-safe workspace file transfers.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aex_hands_protocol::files::{
    FILE_PART_BYTES, FileDownloadChunk, FileDownloadId, FileDownloadState, FileFailureCode,
    FilePartReceipt, FileRequest, FileResponse, FileUploadState, MAX_FILE_BYTES,
};
use aex_hands_protocol::operation::{FileMode, GuestPath, GuestRoot};
use aex_wire::ids::{ContentHash, UploadId};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

const MAX_PARTS: u32 = 10_000;

/// Exact guest file transport state.
#[derive(Debug)]
pub struct FileService {
    root: GuestRoot,
    workspace: PathBuf,
    uploads: PathBuf,
    downloads: Mutex<BTreeMap<FileDownloadId, OpenDownload>>,
}

#[derive(Debug)]
struct OpenDownload {
    file: File,
    path: GuestPath,
    state: FileDownloadState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct UploadManifest {
    state: FileUploadState,
    mode: FileMode,
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
            FileRequest::UploadOpen {
                upload,
                path,
                size_bytes,
                sha256,
                mode,
            } => self.upload_open(upload, path, size_bytes, sha256, mode),
            FileRequest::UploadStatus { upload } => self.upload_status(upload),
            FileRequest::UploadPart {
                upload,
                part_number,
                offset,
                sha256,
                bytes,
            } => self.upload_part(upload, part_number, offset, sha256, &bytes),
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
        }
    }

    fn upload_open(
        &self,
        upload: UploadId,
        path: GuestPath,
        size_bytes: u64,
        sha256: ContentHash,
        mode: FileMode,
    ) -> Result<FileResponse, FileFailureCode> {
        if size_bytes > MAX_FILE_BYTES || path.as_str() == self.root.0 {
            return Err(FileFailureCode::LimitExceeded);
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

    fn upload_status(&self, upload: UploadId) -> Result<FileResponse, FileFailureCode> {
        Ok(FileResponse::Upload {
            state: self.read_manifest(upload)?.state,
        })
    }

    fn upload_part(
        &self,
        upload: UploadId,
        part_number: u32,
        offset: u64,
        sha256: ContentHash,
        bytes: &[u8],
    ) -> Result<FileResponse, FileFailureCode> {
        let mut manifest = self.read_manifest(upload)?;
        if manifest.state.complete {
            return Err(FileFailureCode::Conflict);
        }
        let expected_offset = u64::from(part_number.saturating_sub(1))
            .checked_mul(u64::from(FILE_PART_BYTES))
            .ok_or(FileFailureCode::InvalidRequest)?;
        if part_number == 0
            || part_number > MAX_PARTS
            || offset != expected_offset
            || offset >= manifest.state.size_bytes
            || bytes.is_empty()
            || bytes.len() > FILE_PART_BYTES as usize
        {
            return Err(FileFailureCode::InvalidRequest);
        }
        let expected_size = (manifest.state.size_bytes - offset).min(u64::from(FILE_PART_BYTES));
        if bytes.len() as u64 != expected_size || ContentHash::of(bytes) != sha256 {
            return Err(FileFailureCode::InvalidRequest);
        }
        let receipt = FilePartReceipt {
            part_number,
            offset,
            size_bytes: u32::try_from(bytes.len()).map_err(|_| FileFailureCode::InvalidRequest)?,
            sha256,
        };
        if let Some(existing) = manifest
            .state
            .parts
            .iter()
            .find(|part| part.part_number == part_number)
        {
            if existing != &receipt
                || std::fs::read(self.part_path(upload, part_number))
                    .map_err(|error| map_io(&error))?
                    != bytes
            {
                return Err(FileFailureCode::Conflict);
            }
            return Ok(FileResponse::Upload {
                state: manifest.state,
            });
        }
        write_atomic(&self.part_path(upload, part_number), bytes)?;
        manifest.state.parts.push(receipt);
        manifest.state.parts.sort_by_key(|part| part.part_number);
        self.write_manifest(&manifest)?;
        Ok(FileResponse::Upload {
            state: manifest.state,
        })
    }

    fn upload_complete(&self, upload: UploadId) -> Result<FileResponse, FileFailureCode> {
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
                .div_ceil(u64::from(FILE_PART_BYTES))
        };
        if manifest.state.parts.len() as u64 != expected_parts {
            return Err(FileFailureCode::InvalidRequest);
        }
        for (index, part) in manifest.state.parts.iter().enumerate() {
            let number = u32::try_from(index + 1).map_err(|_| FileFailureCode::InvalidRequest)?;
            if part.part_number != number
                || part.offset != u64::from(number - 1) * u64::from(FILE_PART_BYTES)
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

    fn upload_abort(&self, upload: UploadId) -> Result<FileResponse, FileFailureCode> {
        if self.read_manifest(upload)?.state.complete {
            return Err(FileFailureCode::Conflict);
        }
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
        if file_size_bytes > MAX_FILE_BYTES {
            return Err(FileFailureCode::LimitExceeded);
        }
        let sha256 = hash_file(&mut file)?;
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
            version,
        };
        let mut open = self
            .downloads
            .lock()
            .map_err(|_| FileFailureCode::Unavailable)?;
        if let Some(existing) = open.get(&download) {
            if existing.state != state {
                return Err(FileFailureCode::Conflict);
            }
        } else {
            open.insert(
                download,
                OpenDownload {
                    file,
                    path: path.clone(),
                    state,
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
        if max_bytes == 0 || max_bytes > FILE_PART_BYTES {
            return Err(FileFailureCode::InvalidRequest);
        }
        let mut open = self
            .downloads
            .lock()
            .map_err(|_| FileFailureCode::Unavailable)?;
        let opened = open.get_mut(&download).ok_or(FileFailureCode::NotFound)?;
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
        opened
            .file
            .seek(std::io::SeekFrom::Start(offset))
            .map_err(|error| map_io(&error))?;
        opened
            .file
            .read_exact(&mut bytes)
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
        let mut opened = self
            .downloads
            .lock()
            .map_err(|_| FileFailureCode::Unavailable)?
            .remove(&download)
            .ok_or(FileFailureCode::NotFound)?;
        let descriptor_metadata = opened.file.metadata().map_err(|error| map_io(&error))?;
        let descriptor_sha = hash_file(&mut opened.file)?;
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
        Ok(FileResponse::DownloadClosed {
            download,
            sha256: descriptor_sha,
            version: opened.state.version,
        })
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

    fn upload_directory(&self, upload: UploadId) -> PathBuf {
        self.uploads.join(upload.to_string())
    }
    fn manifest_path(&self, upload: UploadId) -> PathBuf {
        self.upload_directory(upload).join("manifest.json")
    }
    fn part_path(&self, upload: UploadId, number: u32) -> PathBuf {
        self.upload_directory(upload)
            .join(format!("part-{number:05}"))
    }
    fn read_manifest(&self, upload: UploadId) -> Result<UploadManifest, FileFailureCode> {
        let bytes = std::fs::read(self.manifest_path(upload)).map_err(|error| map_io(&error))?;
        serde_json::from_slice(&bytes).map_err(|_| FileFailureCode::Conflict)
    }
    fn write_manifest(&self, manifest: &UploadManifest) -> Result<(), FileFailureCode> {
        let bytes = serde_json::to_vec(manifest).map_err(|_| FileFailureCode::Unavailable)?;
        write_atomic(&self.manifest_path(manifest.state.upload), &bytes)
    }
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
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
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

fn file_version(metadata: &std::fs::Metadata, sha256: ContentHash) -> ContentHash {
    let mut evidence = Vec::with_capacity(128);
    evidence.extend_from_slice(&metadata.len().to_be_bytes());
    evidence.extend_from_slice(sha256.as_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        for value in [
            metadata.dev(),
            metadata.ino(),
            metadata.mtime() as u64,
            metadata.mtime_nsec() as u64,
            metadata.ctime() as u64,
            metadata.ctime_nsec() as u64,
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

#[cfg(test)]
mod tests {
    use super::FileService;
    use aex_hands_protocol::files::{FILE_PART_BYTES, FileDownloadId, FileRequest, FileResponse};
    use aex_hands_protocol::operation::{FileMode, GuestPath, GuestRoot};
    use aex_wire::ids::{ContentHash, PrefixedId as _, UploadId, Uuid7};

    fn upload(seed: u8) -> UploadId {
        UploadId::from_uuid7(Uuid7::compose(1, [seed; 10]))
    }
    fn path(root: &GuestRoot, value: &str) -> GuestPath {
        GuestPath::parse(root, &format!("{}/{value}", root.0)).expect("contained path")
    }

    #[test]
    fn multipart_upload_replays_parts_and_publishes_only_after_whole_sha_verification() {
        let directory = tempfile::tempdir().expect("temporary root");
        let workspace = directory.path().join("workspace");
        let journal = directory.path().join("journal");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let root = GuestRoot::workspace();
        let service = FileService::open(root.clone(), &workspace, &journal).expect("service");
        let mut bytes = vec![b'a'; FILE_PART_BYTES as usize];
        bytes.extend_from_slice(b"tail");
        let identity = upload(1);
        let destination = path(&root, "artifact.bin");
        assert!(matches!(
            service.answer(FileRequest::UploadOpen {
                upload: identity,
                path: destination,
                size_bytes: bytes.len() as u64,
                sha256: ContentHash::of(&bytes),
                mode: FileMode::ReadWrite,
            }),
            FileResponse::Upload { .. }
        ));
        assert!(!workspace.join("artifact.bin").exists());
        let first = FileRequest::UploadPart {
            upload: identity,
            part_number: 1,
            offset: 0,
            sha256: ContentHash::of(&bytes[..FILE_PART_BYTES as usize]),
            bytes: bytes[..FILE_PART_BYTES as usize].to_vec(),
        };
        assert_eq!(service.answer(first.clone()), service.answer(first));
        assert!(matches!(
            service.answer(FileRequest::UploadPart {
                upload: identity,
                part_number: 2,
                offset: u64::from(FILE_PART_BYTES),
                sha256: ContentHash::of(b"tail"),
                bytes: b"tail".to_vec(),
            }),
            FileResponse::Upload { .. }
        ));
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
            if state.start == 1 && state.length_bytes == 3 && state.file_size_bytes == 5));
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
