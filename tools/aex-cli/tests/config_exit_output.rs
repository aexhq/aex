//! Configuration, output, and stable exit-code tests.

use std::collections::BTreeMap;
use std::io::Cursor;

use aex_cli::config::{ConfigInputs, Profile, resolve_config};
use aex_cli::error::{CliErrorClass, exit_code_for_error_class};
use aex_cli::output::{OutputFormat, render_raw_download};
use aex_wire::error::ErrorClass;

#[test]
fn configuration_precedence_is_flag_env_profile_default() {
    let profile = Profile {
        central_url: Some("https://profile.example".into()),
        output: Some(OutputFormat::Text),
        ..Profile::default()
    };
    let env = BTreeMap::from([
        ("AEX_CENTRAL_URL".into(), "https://env.example".into()),
        ("AEX_OUTPUT".into(), "json".into()),
    ]);
    let resolved = resolve_config(ConfigInputs {
        central_url: Some("https://flag.example".into()),
        output: None,
        profile,
        env,
        stdout_is_terminal: true,
    })
    .expect("config resolves");
    assert_eq!(resolved.central_url, "https://flag.example");
    assert_eq!(resolved.output, OutputFormat::Json);
}

#[test]
fn exit_code_table_is_total() {
    let cases = [
        (ErrorClass::Auth, 4),
        (ErrorClass::NotFound, 5),
        (ErrorClass::Conflict, 6),
        (ErrorClass::Precondition, 6),
        (ErrorClass::Validation, 7),
        (ErrorClass::Quota, 8),
        (ErrorClass::State, 9),
        (ErrorClass::Unavailable, 10),
        (ErrorClass::Internal, 11),
    ];
    for (class, expected) in cases {
        assert_eq!(exit_code_for_error_class(class), expected);
    }
    assert_eq!(CliErrorClass::Deadline.exit_code(), 13);
    assert_eq!(CliErrorClass::LocalIo.exit_code(), 14);
    assert_eq!(CliErrorClass::Interrupted.exit_code(), 130);
}

#[test]
fn raw_download_is_byte_pure() {
    let mut stdout = Cursor::new(Vec::new());
    let mut stderr = Cursor::new(Vec::new());
    render_raw_download(&[0, 1, 2, 255], &mut stdout, &mut stderr).expect("write succeeds");
    assert_eq!(stdout.into_inner(), vec![0, 1, 2, 255]);
    assert!(stderr.into_inner().is_empty());
}
