mod common;
use aex_server::{App, account, config::Config};
use axum::{
    body::{Bytes, to_bytes},
    http::Method,
};

#[tokio::test]
async fn verified_accounts_manage_only_their_keys_and_workload_keys_cannot_escalate() {
    let dir = tempfile::tempdir().unwrap();
    let mut config: Config =
        serde_json::from_str(include_str!("../../../examples/config.json")).unwrap();
    config.data_dir = dir.path().into();
    let app = App::open(
        config,
        "b".repeat(32),
        "o".repeat(32),
        common::database(dir.path()).await,
        "s".repeat(32),
    )
    .await
    .unwrap();
    async fn call(
        app: &App,
        method: Method,
        path: &str,
        token: &str,
        body: serde_json::Value,
    ) -> serde_json::Value {
        let response = account::handle(
            app,
            &method,
            path,
            token,
            Bytes::from(serde_json::to_vec(&body).unwrap()),
        )
        .await
        .unwrap();
        serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap()
    }
    let mut sessions = vec![];
    for subject in ["google:a", "google:b"] {
        sessions.push(
            call(
                &app,
                Method::POST,
                "/v1/accounts",
                &"s".repeat(32),
                serde_json::json!({"subject":subject,"email":"verified@example.com"}),
            )
            .await["token"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    let issued = call(
        &app,
        Method::POST,
        "/v1/keys",
        &sessions[0],
        serde_json::json!({"name":"first"}),
    )
    .await;
    let token = issued["token"].as_str().unwrap();
    let id = issued["key"]["id"].as_str().unwrap();
    assert!(app.store.principal(token).await.is_ok());
    for (method, path, credential, body) in [
        (
            Method::GET,
            "/v1/keys".to_owned(),
            token,
            serde_json::json!({}),
        ),
        (
            Method::POST,
            "/v1/keys".to_owned(),
            token,
            serde_json::json!({"name":"escalate"}),
        ),
        (
            Method::PATCH,
            format!("/v1/keys/{id}"),
            sessions[1].as_str(),
            serde_json::json!({"name":"stolen"}),
        ),
        (
            Method::DELETE,
            format!("/v1/keys/{id}"),
            sessions[1].as_str(),
            serde_json::json!({}),
        ),
        (
            Method::POST,
            "/v1/accounts".to_owned(),
            token,
            serde_json::json!({"subject":"google:c","email":"c@example.com"}),
        ),
    ] {
        assert!(
            account::handle(
                &app,
                &method,
                &path,
                credential,
                Bytes::from(serde_json::to_vec(&body).unwrap())
            )
            .await
            .is_err()
        );
    }
    let renamed = call(
        &app,
        Method::PATCH,
        &format!("/v1/keys/{id}"),
        &sessions[0],
        serde_json::json!({"name":"renamed"}),
    )
    .await;
    assert_eq!(renamed["name"], "renamed");
    assert!(
        !call(
            &app,
            Method::GET,
            "/v1/keys",
            &sessions[0],
            serde_json::json!({})
        )
        .await
        .to_string()
        .contains(token)
    );
    assert_eq!(
        call(
            &app,
            Method::GET,
            "/v1/account",
            token,
            serde_json::json!({})
        )
        .await["billing"],
        "preview_customer_model_keys"
    );
    account::handle(
        &app,
        &Method::DELETE,
        &format!("/v1/keys/{id}"),
        &sessions[0],
        Bytes::new(),
    )
    .await
    .unwrap();
    assert!(app.store.principal(token).await.is_err());
    account::handle(
        &app,
        &Method::DELETE,
        "/v1/account/session",
        &sessions[0],
        Bytes::new(),
    )
    .await
    .unwrap();
    assert!(
        account::handle(&app, &Method::GET, "/v1/keys", &sessions[0], Bytes::new())
            .await
            .is_err()
    );
}
