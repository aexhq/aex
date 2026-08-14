//! Configuration, output, and stable exit-code tests.

use std::collections::BTreeMap;
use std::io::Cursor;

use aex_cli::config::{
    ConfigInputs, Profile, get_value, load_profile, resolve_api_key, resolve_config,
    resolve_dashboard_session, set_value,
};
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
        regional_url: Some("https://regional.example".into()),
        output: None,
        profile,
        env,
        stdout_is_terminal: true,
    })
    .expect("config resolves");
    assert_eq!(resolved.central_url, "https://flag.example");
    assert_eq!(resolved.regional_url, "https://regional.example");
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

#[test]
fn config_persists_only_non_secret_values_and_protected_references() {
    let directory = tempfile::tempdir().expect("temp directory");
    let path = directory.path().join("config.json");
    set_value(&path, "default", "central-url", "https://api.example").expect("URL");
    set_value(&path, "default", "api-key-ref", "env:AEX_API_KEY_DEV").expect("reference");
    set_value(
        &path,
        "default",
        "dashboard-session-ref",
        "env:AEX_DASHBOARD_SESSION_DEV",
    )
    .expect("dashboard reference");
    assert!(set_value(&path, "default", "api-key", "visible-secret").is_err());
    assert!(set_value(&path, "default", "provider-key", "visible-secret").is_err());
    assert!(set_value(&path, "default", "mcp-secret", "visible-secret").is_err());

    let bytes = std::fs::read_to_string(&path).expect("config bytes");
    assert!(!bytes.contains("visible-secret"));
    let profile = load_profile(&path, "default").expect("profile");
    assert_eq!(
        get_value(&profile, "api-key-ref")
            .expect("known key")
            .as_deref(),
        Some("env:AEX_API_KEY_DEV")
    );
}

#[test]
fn api_key_resolution_prefers_environment_and_debug_never_contains_material() {
    let secret = "aex_wk_secret_value_that_must_not_appear";
    let profile = Profile {
        api_key_ref: Some("env:AEX_API_KEY_DEV".to_owned()),
        ..Profile::default()
    };
    let env = BTreeMap::from([("AEX_API_KEY_DEV".to_owned(), secret.to_owned())]);
    let resolved = resolve_api_key(&profile, &env).expect("reference resolves");
    assert_eq!(resolved.as_str(), secret);
    assert!(!format!("{profile:?}").contains(secret));
}

#[test]
fn central_and_regional_credentials_are_separate_and_missing_central_fails_locally() {
    let workspace_key = "aex_wk_workspace_material";
    let dashboard_session = "aex_ds_dashboard_material";
    let profile = Profile {
        api_key_ref: Some("env:AEX_API_KEY_DEV".to_owned()),
        dashboard_session_ref: Some("env:AEX_DASHBOARD_SESSION_DEV".to_owned()),
        ..Profile::default()
    };
    let env = BTreeMap::from([
        ("AEX_API_KEY_DEV".to_owned(), workspace_key.to_owned()),
        (
            "AEX_DASHBOARD_SESSION_DEV".to_owned(),
            dashboard_session.to_owned(),
        ),
    ]);
    assert_eq!(
        resolve_api_key(&profile, &env).expect("regional").as_str(),
        workspace_key
    );
    assert_eq!(
        resolve_dashboard_session(&profile, &env)
            .expect("central")
            .as_str(),
        dashboard_session
    );
    let missing = BTreeMap::from([("AEX_API_KEY_DEV".to_owned(), workspace_key.to_owned())]);
    assert!(resolve_dashboard_session(&profile, &missing).is_err());

    let crossed = BTreeMap::from([
        ("AEX_API_KEY".to_owned(), dashboard_session.to_owned()),
        ("AEX_DASHBOARD_SESSION".to_owned(), workspace_key.to_owned()),
    ]);
    assert!(resolve_api_key(&Profile::default(), &crossed).is_err());
    assert!(resolve_dashboard_session(&Profile::default(), &crossed).is_err());
}
