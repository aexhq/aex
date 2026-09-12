use crate::error::{Error, Result};
use bytes::Bytes;
use futures_util::{Stream, future::BoxFuture};
use std::{pin::Pin, time::Duration};

pub type ObjectStream = Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>;

pub trait Storage: Send + Sync {
    fn put_new<'a>(
        &'a self,
        key: &'a str,
        content_type: &'a str,
        bytes: Bytes,
    ) -> BoxFuture<'a, Result<()>>;
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<ObjectStream>>;
    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<()>>;
}

pub struct S3 {
    client: aws_sdk_s3::Client,
    bucket: String,
}

impl S3 {
    pub fn new(config: &super::Config) -> anyhow::Result<Self> {
        use aws_sdk_s3::config::{Credentials, Region, retry::RetryConfig, timeout::TimeoutConfig};
        let access = std::env::var("AEX_S3_ACCESS_KEY_ID")?;
        let secret = std::env::var("AEX_S3_SECRET_ACCESS_KEY")?;
        anyhow::ensure!(
            !access.is_empty() && !secret.is_empty(),
            "attachment storage credentials are required"
        );
        let mut builder = aws_sdk_s3::Config::builder()
            .behavior_version_latest()
            .region(Region::new(config.region.clone()))
            .credentials_provider(Credentials::new(
                access,
                secret,
                std::env::var("AEX_S3_SESSION_TOKEN")
                    .ok()
                    .filter(|value| !value.is_empty()),
                None,
                "aex-attachments",
            ))
            .force_path_style(true)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .timeout_config(
                TimeoutConfig::builder()
                    .operation_timeout(Duration::from_secs(config.transfer_timeout_secs))
                    .build(),
            );
        if let Some(endpoint) = &config.endpoint {
            builder = builder.endpoint_url(endpoint);
        }
        Ok(Self {
            client: aws_sdk_s3::Client::from_conf(builder.build()),
            bucket: config.bucket.clone(),
        })
    }
}

impl Storage for S3 {
    fn put_new<'a>(
        &'a self,
        key: &'a str,
        content_type: &'a str,
        bytes: Bytes,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.client
                .put_object()
                .bucket(&self.bucket)
                .key(key)
                .content_type(content_type)
                .if_none_match("*")
                .body(bytes.into())
                .send()
                .await
                .map_err(|_| Error::ambiguous())?;
            Ok(())
        })
    }

    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<ObjectStream>> {
        Box::pin(async move {
            let mut object = self
                .client
                .get_object()
                .bucket(&self.bucket)
                .key(key)
                .send()
                .await
                .map_err(|error| {
                    if error
                        .as_service_error()
                        .is_some_and(|error| error.is_no_such_key())
                    {
                        Error::missing()
                    } else {
                        Error::internal()
                    }
                })?;
            let stream: ObjectStream = Box::pin(async_stream::try_stream! {
                while let Some(chunk) = object.body.next().await {
                    yield chunk.map_err(|_| Error::internal())?;
                }
            });
            Ok(stream)
        })
    }

    fn delete<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.client
                .delete_object()
                .bucket(&self.bucket)
                .key(key)
                .send()
                .await
                .map_err(|_| Error::ambiguous())?;
            Ok(())
        })
    }
}
