mod common;

use aex_server::{
    App,
    attachments::{
        self, Attachment,
        storage::{ObjectStream, Storage},
    },
    error::{Error, Result},
};
use axum::{
    body::{Body, Bytes, to_bytes},
    http::{Method, Request, StatusCode},
    response::Response,
};
use futures_util::future::BoxFuture;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tower::ServiceExt;

#[derive(Default)]
struct Objects {
    bytes: Mutex<HashMap<String, Bytes>>,
    uncertain_put: AtomicBool,
    fail_delete: AtomicBool,
    stall_get: AtomicBool,
    stall_body: AtomicBool,
    pause_put: AtomicBool,
    written: tokio::sync::Notify,
    resume: tokio::sync::Notify,
}

impl Storage for Objects {
    fn put_new<'a>(&'a self, key: &'a str, _: &'a str, bytes: Bytes) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            {
                let mut objects = self.bytes.lock().unwrap();
                if objects.contains_key(key) {
                    return Err(Error::conflict("object exists"));
                }
                objects.insert(key.into(), bytes);
            }
            self.written.notify_one();
            if self.pause_put.load(Ordering::SeqCst) {
                self.resume.notified().await;
            }
            if self.uncertain_put.load(Ordering::SeqCst) {
                return Err(Error::ambiguous());
            }
            Ok(())
        })
    }
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<ObjectStream>> {
        Box::pin(async move {
            if self.stall_get.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            if self.stall_body.load(Ordering::SeqCst) {
                return Ok(Box::pin(futures_util::stream::pending()) as ObjectStream);
            }
            let bytes = self
                .bytes
                .lock()
                .unwrap()
                .get(key)
                .cloned()
                .ok_or_else(Error::missing)?;
            let stream: ObjectStream = Box::pin(futures_util::stream::once(async { Ok(bytes) }));
            Ok(stream)
        })
    }
    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if self.fail_delete.load(Ordering::SeqCst) {
                return Err(Error::ambiguous());
            }
            self.bytes.lock().unwrap().remove(key);
            Ok(())
        })
    }
}

async fn fixture(path: &std::path::Path) -> (App, Arc<Objects>, String, String, String) {
    let mut config: aex_server::config::Config =
        serde_json::from_str(include_str!("../../../examples/config.json")).unwrap();
    config.data_dir = path.into();
    let mut app = App::open(
        config.clone(),
        "b".repeat(32),
        "o".repeat(32),
        common::database(path).await,
        "s".repeat(32),
    )
    .await
    .unwrap();
    config.attachments = Some(attachments::Config {
        public_origin: "https://media.example.com".into(),
        bucket: "test".into(),
        region: "us-east-1".into(),
        endpoint: None,
        max_bytes: 1024,
        count_per_account: 2,
        bytes_per_account: 2048,
        ttl_secs: 60,
        transfer_timeout_secs: 5,
    });
    config.validate().unwrap();
    app.config = Arc::new(config);
    let objects = Arc::new(Objects::default());
    app.attachment_storage = Some(objects.clone());
    app.accepting.store(true, Ordering::SeqCst);
    let account = app.store.create_account(&app.config.limits).await.unwrap();
    let (_, token) = app
        .store
        .issue_key(&account, &app.config.limits)
        .await
        .unwrap();
    let second = app.store.create_account(&app.config.limits).await.unwrap();
    let (_, other) = app
        .store
        .issue_key(&second, &app.config.limits)
        .await
        .unwrap();
    sqlx::query("INSERT INTO sessions(id,account,created) VALUES('session',$1,$2)")
        .bind(&account)
        .bind(aex_server::store::now())
        .execute(&app.store.0)
        .await
        .unwrap();
    (app, objects, account, token, other)
}

async fn call(
    app: &App,
    method: Method,
    path: &str,
    token: Option<&str>,
    key: &str,
    bytes: &'static [u8],
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("idempotency-key", key)
        .header("content-type", "application/pdf");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    aex_server::http::router(app.clone())
        .oneshot(request.body(Body::from(bytes)).unwrap())
        .await
        .unwrap()
}

async fn attachment(response: Response) -> Attachment {
    assert!(response.status().is_success());
    serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap()
}

fn read_path(attachment: &Attachment) -> String {
    let url = url::Url::parse(attachment.media.url()).unwrap();
    format!("{}?{}", url.path(), url.query().unwrap())
}

const UPLOAD: &str = "/v1/sessions/session/attachments";
const PDF: &[u8] = b"%PDF-1.7\nfixture";

#[tokio::test]
async fn stalled_or_truncated_downloads_fail_and_release_transfer_admission() {
    let directory = tempfile::tempdir().unwrap();
    let (mut app, objects, account, token, _) = fixture(directory.path()).await;
    Arc::make_mut(&mut app.config)
        .attachments
        .as_mut()
        .unwrap()
        .transfer_timeout_secs = 1;
    let file = attachment(call(&app, Method::POST, UPLOAD, Some(&token), "file", PDF).await).await;
    let path = read_path(&file);
    let account_requests = app.account_requests.lock().await[&account].clone();
    objects.stall_get.store(true, Ordering::SeqCst);
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        call(&app, Method::GET, &path, None, "", b""),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    objects.stall_get.store(false, Ordering::SeqCst);
    objects.stall_body.store(true, Ordering::SeqCst);
    let response = call(&app, Method::GET, &path, None, "", b"").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            to_bytes(response.into_body(), 1024)
        )
        .await
        .unwrap()
        .is_err()
    );
    objects.stall_body.store(false, Ordering::SeqCst);
    for bytes in [
        Bytes::from_static(b"short"),
        Bytes::from(vec![0; PDF.len() + 1]),
    ] {
        objects
            .bytes
            .lock()
            .unwrap()
            .insert(format!("attachments/{}", file.id), bytes);
        let response = call(&app, Method::GET, &path, None, "", b"").await;
        assert!(to_bytes(response.into_body(), 1024).await.is_err());
    }
    assert_eq!(app.requests.available_permits(), app.config.limits.requests);
    assert_eq!(
        account_requests.available_permits(),
        app.config.limits.requests_per_account
    );
}

#[tokio::test]
async fn ordinary_requests_leave_download_capacity_and_dropped_streams_release_it() {
    let directory = tempfile::tempdir().unwrap();
    let (app, _, account, token, _) = fixture(directory.path()).await;
    let file = attachment(call(&app, Method::POST, UPLOAD, Some(&token), "file", PDF).await).await;
    let account_requests = app.account_requests.lock().await[&account].clone();
    let global: Vec<_> = (0..app.config.limits.requests - 1)
        .map(|_| aex_server::admission::request(app.requests.clone(), true).unwrap())
        .collect();
    let account: Vec<_> = (0..app.config.limits.requests_per_account - 1)
        .map(|_| aex_server::admission::request(account_requests.clone(), true).unwrap())
        .collect();
    assert!(aex_server::admission::request(app.requests.clone(), true).is_err());
    assert!(aex_server::admission::request(account_requests.clone(), true).is_err());
    let response = call(&app, Method::GET, &read_path(&file), None, "", b"").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(app.requests.available_permits(), 0);
    drop(response);
    assert_eq!(app.requests.available_permits(), 1);
    assert_eq!(account_requests.available_permits(), 1);
    drop((global, account));
}

#[tokio::test]
async fn session_deletion_during_upload_prevents_publication_and_maintenance_defers_active_uploads()
{
    let directory = tempfile::tempdir().unwrap();
    let (app, objects, _, token, _) = fixture(directory.path()).await;
    objects.pause_put.store(true, Ordering::SeqCst);
    let running = tokio::spawn({
        let app = app.clone();
        async move { call(&app, Method::POST, UPLOAD, Some(&token), "race", PDF).await }
    });
    objects.written.notified().await;
    assert_eq!(attachments::maintain(&app).await.unwrap().deleted, 0);
    app.store.mark_deleting("session").await.unwrap();
    objects.resume.notify_one();
    assert_eq!(running.await.unwrap().status(), StatusCode::CONFLICT);
    let state: String = sqlx::query_scalar("SELECT state FROM attachments")
        .fetch_one(&app.store.0)
        .await
        .unwrap();
    assert_eq!(state, "deleting");
    assert_eq!(attachments::maintain(&app).await.unwrap().deleted, 1);
}

struct DiskObject(std::path::PathBuf);

impl Storage for DiskObject {
    fn put_new<'a>(&'a self, key: &'a str, _: &'a str, bytes: Bytes) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let path = self.0.join(key);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(path)
                .unwrap();
            std::io::Write::write_all(&mut file, &bytes).unwrap();
            file.sync_all().unwrap();
            std::fs::write(self.0.join("written"), key).unwrap();
            futures_util::future::pending().await
        })
    }
    fn get<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<ObjectStream>> {
        Box::pin(async { panic!("a pending object must not be read") })
    }
    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            std::fs::remove_file(self.0.join(key)).unwrap();
            Ok(())
        })
    }
}

#[tokio::test]
async fn process_death_after_object_write_preserves_pending_reservation() {
    if let Ok(directory) = std::env::var("AEX_ATTACHMENT_CRASH_DIRECTORY") {
        let path = std::path::Path::new(&directory);
        let (mut app, _, _, token, _) = fixture(path).await;
        app.attachment_storage = Some(Arc::new(DiskObject(path.into())));
        call(
            &app,
            Method::POST,
            UPLOAD,
            Some(&token),
            "process-crash",
            PDF,
        )
        .await;
        panic!("parent must kill this process before publication");
    }
    let directory = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "process_death_after_object_write_preserves_pending_reservation",
        ])
        .env("AEX_ATTACHMENT_CRASH_DIRECTORY", directory.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    while !directory.path().join("written").exists() {
        if tokio::time::Instant::now() >= deadline || child.try_wait().unwrap().is_some() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child did not reach the object-write boundary");
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let key = std::fs::read_to_string(directory.path().join("written")).unwrap();
    assert_eq!(std::fs::read(directory.path().join(&key)).unwrap(), PDF);
    let mut config: aex_server::config::Config =
        serde_json::from_str(include_str!("../../../examples/config.json")).unwrap();
    config.data_dir = directory.path().into();
    let mut app = App::open(
        config.clone(),
        "b".repeat(32),
        "o".repeat(32),
        common::database(directory.path()).await,
        "s".repeat(32),
    )
    .await
    .unwrap();
    let state: (String, i64) =
        sqlx::query_as("SELECT state,bytes FROM attachments WHERE object_key=$1")
            .bind(&key)
            .fetch_one(&app.store.0)
            .await
            .unwrap();
    assert_eq!(state, ("pending".into(), PDF.len() as i64));
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM claims WHERE state='pending'")
        .fetch_one(&app.store.0)
        .await
        .unwrap();
    assert_eq!(pending, 1);
    config.attachments = Some(attachments::Config {
        public_origin: "https://example.com".into(),
        bucket: "test".into(),
        region: "us-east-1".into(),
        endpoint: None,
        max_bytes: 1024,
        count_per_account: 2,
        bytes_per_account: 2048,
        ttl_secs: 60,
        transfer_timeout_secs: 5,
    });
    app.config = Arc::new(config);
    app.attachment_storage = Some(Arc::new(DiskObject(directory.path().into())));
    assert_eq!(attachments::maintain(&app).await.unwrap().deleted, 1);
    assert!(!directory.path().join(key).exists());
}

#[tokio::test]
async fn urls_are_scoped_immutable_and_expire_without_waiting_for_object_deletion() {
    let directory = tempfile::tempdir().unwrap();
    let (app, objects, account, token, other) = fixture(directory.path()).await;
    assert_eq!(
        call(&app, Method::POST, UPLOAD, Some(&other), "upload", PDF)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            Method::POST,
            UPLOAD,
            Some(&token),
            "invalid",
            b"not a PDF"
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let first =
        attachment(call(&app, Method::POST, UPLOAD, Some(&token), "upload", PDF).await).await;
    let repeated =
        attachment(call(&app, Method::POST, UPLOAD, Some(&token), "upload", PDF).await).await;
    assert_eq!(first.media, repeated.media);
    assert_eq!(first.expires_at, repeated.expires_at);
    assert_eq!(objects.bytes.lock().unwrap().len(), 1);
    assert_eq!(
        call(
            &app,
            Method::POST,
            UPLOAD,
            Some(&token),
            "upload",
            b"%PDF-1.7\nchanged"
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let path = read_path(&first);
    let response = call(&app, Method::GET, &path, None, "", b"").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/pdf");
    assert_eq!(response.headers()["cache-control"], "private, no-store");
    assert_eq!(to_bytes(response.into_body(), 1024).await.unwrap(), PDF);
    assert_eq!(app.requests.available_permits(), app.config.limits.requests);
    assert_eq!(
        call(&app, Method::HEAD, &path, None, "", b"")
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            Method::GET,
            &format!("/v1/attachments/{}/content?token=forged", first.id),
            None,
            "",
            b""
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    app.store.set_account_active(&account, false).await.unwrap();
    assert_eq!(
        call(&app, Method::GET, &path, None, "", b"").await.status(),
        StatusCode::NOT_FOUND
    );
    app.store.set_account_active(&account, true).await.unwrap();
    sqlx::query("UPDATE attachments SET expires_at=$1 WHERE id=$2")
        .bind(aex_server::store::now())
        .bind(&first.id)
        .execute(&app.store.0)
        .await
        .unwrap();
    assert_eq!(
        call(&app, Method::GET, &path, None, "", b"").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(objects.bytes.lock().unwrap().len(), 1);
    assert_eq!(attachments::maintain(&app).await.unwrap().deleted, 1);
    assert!(objects.bytes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn uncertain_uploads_keep_reservations_and_cleanup_survives_store_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let (mut app, objects, _, token, _) = fixture(directory.path()).await;
    objects.uncertain_put.store(true, Ordering::SeqCst);
    assert_eq!(
        call(&app, Method::POST, UPLOAD, Some(&token), "lost", PDF)
            .await
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        call(&app, Method::POST, UPLOAD, Some(&token), "lost", PDF)
            .await
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        objects.bytes.lock().unwrap().len(),
        1,
        "an uncertain effect is not re-executed"
    );
    assert_eq!(
        attachments::maintain(&app).await.unwrap().deleted,
        0,
        "pending effects require drained reconciliation"
    );
    app.store = aex_server::store::Store::open(&common::database(directory.path()).await)
        .await
        .unwrap();
    app.accepting.store(false, Ordering::SeqCst);
    objects.fail_delete.store(true, Ordering::SeqCst);
    assert_eq!(attachments::maintain(&app).await.unwrap().failed, 1);
    let reserved: i64 = sqlx::query_scalar("SELECT sum(bytes)::bigint FROM attachments")
        .fetch_one(&app.store.0)
        .await
        .unwrap();
    assert_eq!(reserved, PDF.len() as i64);
    objects.fail_delete.store(false, Ordering::SeqCst);
    assert_eq!(attachments::maintain(&app).await.unwrap().deleted, 1);
    assert!(objects.bytes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn account_quota_is_atomic_and_session_deletion_revokes_reads() {
    let directory = tempfile::tempdir().unwrap();
    let (app, objects, _, token, other) = fixture(directory.path()).await;
    let (one, two, three) = tokio::join!(
        call(&app, Method::POST, UPLOAD, Some(&token), "one", PDF),
        call(&app, Method::POST, UPLOAD, Some(&token), "two", PDF),
        call(&app, Method::POST, UPLOAD, Some(&token), "three", PDF),
    );
    let mut responses = vec![one, two, three];
    assert_eq!(
        responses
            .iter()
            .filter(|r| r.status() == StatusCode::CREATED)
            .count(),
        2
    );
    assert_eq!(
        responses
            .iter()
            .filter(|r| r.status() == StatusCode::SERVICE_UNAVAILABLE)
            .count(),
        1
    );
    let response = responses.swap_remove(
        responses
            .iter()
            .position(|r| r.status().is_success())
            .unwrap(),
    );
    let first = attachment(response).await;
    let delete = format!("{UPLOAD}/{}", first.id);
    assert_eq!(
        call(&app, Method::DELETE, &delete, Some(&other), "", b"")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, Method::DELETE, &delete, Some(&token), "", b"")
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&app, Method::GET, &read_path(&first), None, "", b"")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    app.store.mark_deleting("session").await.unwrap();
    assert_eq!(attachments::maintain(&app).await.unwrap().deleted, 2);
    assert!(objects.bytes.lock().unwrap().is_empty());
}
