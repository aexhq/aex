#![cfg(feature = "storage-integration-tests")]

use aex_server::attachments::{
    Config,
    storage::{S3, Storage},
};
use bytes::Bytes;
use futures_util::{FutureExt, StreamExt};

#[tokio::test]
async fn s3_preserves_immutable_objects_and_deletion_is_repeatable() {
    let storage = S3::new(&Config {
        bucket: std::env::var("AEX_TEST_S3_BUCKET").expect("AEX_TEST_S3_BUCKET is required"),
        region: std::env::var("AEX_TEST_S3_REGION").expect("AEX_TEST_S3_REGION is required"),
        endpoint: std::env::var("AEX_TEST_S3_ENDPOINT").ok(),
        public_origin: "https://example.com".into(),
        max_bytes: 1024,
        count_per_account: 1,
        bytes_per_account: 1024,
        ttl_secs: 60,
        transfer_timeout_secs: 30,
    })
    .unwrap();
    let key = format!(
        "attachments/integration-{}",
        aex_server::identity::random("test")
    );
    let bytes = Bytes::from_static(b"%PDF-1.7\nimmutable fixture");
    storage
        .put_new(&key, "application/pdf", bytes.clone())
        .await
        .unwrap();
    let result = std::panic::AssertUnwindSafe(async {
        assert!(
            storage
                .put_new(&key, "application/pdf", Bytes::from_static(b"changed"))
                .await
                .is_err()
        );
        let mut stream = storage.get(&key).await.unwrap();
        let mut read = Vec::new();
        while let Some(chunk) = stream.next().await {
            read.extend(chunk.unwrap());
        }
        assert_eq!(read, bytes);
    });
    // Cleanup also runs if an assertion fails; the bucket may contain other test runs.
    let checked = result.catch_unwind().await;
    storage.delete(&key).await.unwrap();
    storage.delete(&key).await.unwrap();
    assert!(storage.get(&key).await.is_err());
    checked.unwrap();
}
