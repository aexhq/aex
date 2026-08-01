//! Local atomic-download planning tests.

use std::fs;

use aex_cli::download::{DownloadDisposition, prepare_download};
use tempfile::tempdir;

#[test]
fn download_uses_same_directory_part_file_and_refuses_overwrite() {
    let directory = tempdir().expect("temp directory");
    let target = directory.path().join("artifact.bin");
    let plan = prepare_download(&target, false, false).expect("fresh target");
    assert_eq!(plan.part_path, directory.path().join("artifact.bin.part"));
    assert_eq!(plan.disposition, DownloadDisposition::Create);

    fs::write(&target, b"existing").expect("fixture target");
    assert!(prepare_download(&target, false, false).is_err());
    assert!(prepare_download(&target, true, false).is_ok());
}

#[test]
fn resume_requires_a_part_and_starts_at_its_exact_length() {
    let directory = tempdir().expect("temp directory");
    let target = directory.path().join("artifact.bin");
    assert!(prepare_download(&target, false, true).is_err());
    fs::write(directory.path().join("artifact.bin.part"), b"prefix").expect("part fixture");
    let plan = prepare_download(&target, false, true).expect("resume plan");
    assert_eq!(plan.disposition, DownloadDisposition::Resume { offset: 6 });
}
