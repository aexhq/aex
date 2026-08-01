#![allow(
    missing_docs,
    reason = "the public planning behavior is documented on its entry point"
)]

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadDisposition {
    Create,
    Replace,
    Resume { offset: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadPlan {
    pub target_path: PathBuf,
    pub part_path: PathBuf,
    pub disposition: DownloadDisposition,
}

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("target already exists; pass --force")]
    TargetExists,
    #[error("resume requires an existing .part file")]
    MissingPart,
    #[error("cannot inspect part file: {0}")]
    Inspect(#[from] std::io::Error),
}

/// Build a same-directory part-file plan without minting a network grant.
///
/// # Errors
///
/// Returns an error when overwrite was not authorized, a resume part is missing,
/// or the existing part cannot be inspected.
pub fn prepare_download(
    target: &Path,
    force: bool,
    resume: bool,
) -> Result<DownloadPlan, DownloadError> {
    let file_name = target
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("download");
    let part_path = target.with_file_name(format!("{file_name}.part"));
    if resume {
        if !part_path.exists() {
            return Err(DownloadError::MissingPart);
        }
        let offset = fs::metadata(&part_path)?.len();
        return Ok(DownloadPlan {
            target_path: target.to_owned(),
            part_path,
            disposition: DownloadDisposition::Resume { offset },
        });
    }
    if target.exists() && !force {
        return Err(DownloadError::TargetExists);
    }
    let disposition = if force {
        DownloadDisposition::Replace
    } else {
        DownloadDisposition::Create
    };
    Ok(DownloadPlan {
        target_path: target.to_owned(),
        part_path,
        disposition,
    })
}
