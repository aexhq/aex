mod common;
use aex_server::{App, config::Config, http};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

async fn call(app: &App, path: &str, token: &str, body: Value) -> (u16, Value) {
    let mut request = Request::post(path).header("content-type", "application/json");
    if !token.is_empty() {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = http::router(app.clone())
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body = to_bytes(response.into_body(), 16384).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn browser_grants_require_account_auth_and_exchange_once_with_pkce() {
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
    app.accepting
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (status, session) = call(
        &app,
        "/v1/accounts",
        &"s".repeat(32),
        json!({"subject":"google:cli", "email":"cli@example.com"}),
    )
    .await;
    assert_eq!(status, 200, "{session}");
    let browser = session["token"].as_str().unwrap();
    let (_, key) = call(&app, "/v1/keys", browser, json!({"name":"workload"})).await;
    let verifier = "v".repeat(43);
    let grant = json!({"code_challenge":URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),"redirect_uri":"http://127.0.0.1:34567/callback"});
    for token in ["", key["token"].as_str().unwrap()] {
        assert_eq!(
            call(&app, "/v1/auth/grants", token, grant.clone()).await.0,
            401
        );
    }
    for redirect in [
        "https://evil.example/callback",
        "http://127.0.0.1:34567/callback?x=1",
        "http://localhost:34567/callback",
        "http://127.0.0.1:34567/other",
    ] {
        let mut bad = grant.clone();
        bad["redirect_uri"] = json!(redirect);
        assert_eq!(call(&app, "/v1/auth/grants", browser, bad).await.0, 400);
    }
    let (status, code) = call(&app, "/v1/auth/grants", browser, grant.clone()).await;
    assert_eq!(status, 200);
    let exchange =
        json!({"code":code["code"],"code_verifier":verifier,"redirect_uri":grant["redirect_uri"]});
    let mut wrong = exchange.clone();
    wrong["code_verifier"] = json!("w".repeat(43));
    assert_eq!(call(&app, "/v1/auth/exchange", "", wrong).await.0, 401);
    let mut wrong = exchange.clone();
    wrong["redirect_uri"] = json!("http://127.0.0.1:34568/callback");
    assert_eq!(call(&app, "/v1/auth/exchange", "", wrong).await.0, 401);
    let (a, b) = tokio::join!(
        call(&app, "/v1/auth/exchange", "", exchange.clone()),
        call(&app, "/v1/auth/exchange", "", exchange)
    );
    assert!((a.0 == 200 && b.0 == 401) || (b.0 == 200 && a.0 == 401));
    let cli = if a.0 == 200 { a.1 } else { b.1 };
    assert_ne!(cli["token"], session["token"]);
    assert_eq!(
        call(
            &app,
            "/v1/keys",
            cli["token"].as_str().unwrap(),
            json!({"name":"from-cli"})
        )
        .await
        .0,
        201
    );
    let (_, code) = call(&app, "/v1/auth/grants", browser, grant.clone()).await;
    sqlx::query("UPDATE login_grants SET expires=0")
        .execute(&app.store.0)
        .await
        .unwrap();
    assert_eq!(call(&app, "/v1/auth/exchange", "", json!({"code":code["code"],"code_verifier":verifier,"redirect_uri":grant["redirect_uri"]})).await.0, 401);
    let (_, code) = call(&app, "/v1/auth/grants", browser, grant.clone()).await;
    let request = Request::delete("/v1/account/session")
        .header("authorization", format!("Bearer {browser}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        http::router(app.clone())
            .oneshot(request)
            .await
            .unwrap()
            .status(),
        204
    );
    assert_eq!(call(&app, "/v1/auth/exchange", "", json!({"code":code["code"],"code_verifier":verifier,"redirect_uri":grant["redirect_uri"]})).await.0, 401);
    assert_eq!(
        call(
            &app,
            "/v1/keys",
            cli["token"].as_str().unwrap(),
            json!({"name":"still-cli"})
        )
        .await
        .0,
        201
    );
    sqlx::query("UPDATE accounts SET active=0")
        .execute(&app.store.0)
        .await
        .unwrap();
    assert_eq!(
        call(
            &app,
            "/v1/keys",
            cli["token"].as_str().unwrap(),
            json!({"name":"suspended"})
        )
        .await
        .0,
        401
    );
}
