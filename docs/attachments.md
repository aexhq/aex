# Attachments

Aex stores session-owned images and PDFs and returns a stable HTTPS URL for Brain. Uploading and
sending are separate operations: publication can fail independently of the model turn. Ended
sessions retain their files until expiry or deletion; deleting a session revokes its files.

| Operation | Authorization and result |
| --- | --- |
| `POST /v1/sessions/{session}/attachments` | Workload key for the owning account; raw bytes, `Content-Type`, required `Idempotency-Key`, optional `x-aex-expires-at` in Unix seconds. Returns `201` with `{ id, media, expires_at }`; completed replay returns `200`. |
| `GET` or `HEAD /v1/attachments/{id}/content?token=…` | The scoped read capability in the URL. Ready, unexpired files require an active account and an owned session. No account Authorization header is needed. |
| `DELETE /v1/sessions/{session}/attachments/{id}` | Owning account key. Revokes new reads immediately and queues deletion; repeat deletion returns `204`. |

PNG, JPEG, GIF, WebP and PDF are supported. The declared type must match the file signature. Empty
or oversized bodies, unsupported types and out-of-range expiry fail before storage. Providers
validate the complete file and enforce their own image, page, size and context limits.

```ts
const attachment = await aex.attachments.upload(session.id, bytes, {
  contentType: "application/pdf",
  idempotencyKey: "report-upload-1",
  signal,
});
await session.send({ message: "Explain the chart", media: [attachment.media] });
await aex.attachments.delete(session.id, attachment.id);
```

`expiresAt` may request an earlier future deadline than the deployment TTL. Expiry and bytes are
immutable after publication. The same upload key replays the original URL and deadline; different
bytes, type or requested expiry conflict. Retrying a completed upload does not extend access or
resurrect a revoked file. Keep returned URLs private: they are bearer capabilities and remain
sensitive when stored in session history or application logs.

Aex streams bytes from storage and returns `Content-Type`, `Content-Length`, `nosniff` and
`Cache-Control: private, no-store`. It does not redirect to the bucket. Expiry denies new fetches;
an already-authorized transfer may finish. Revocation cannot retract bytes a provider has fetched.
A later Brain turn can fail if its retained file has expired. Brain never refreshes or uploads it.

Account usage reports `attachments` and `attachment_bytes` separately from local journal
`retained_bytes`. Pending, ready and deleting objects consume quota until confirmed cleanup.
Uploads reserve quota transactionally before one storage write. An interrupted or uncertain write
remains pending and is never automatically repeated. Operator maintenance reconciles pending
uploads while admission is drained and removes expired/revoked files in bounded batches. Failed
deletion keeps its reservation for a later maintenance pass.

Configure the optional `attachments` object using the generated configuration schema: public
HTTPS origin, bucket, region, optional S3-compatible HTTPS endpoint, upload/account limits, TTL and
transfer timeout. Supply `AEX_S3_ACCESS_KEY_ID` and `AEX_S3_SECRET_ACCESS_KEY` only to the control
service; temporary credentials may also supply `AEX_S3_SESSION_TOKEN`. With no attachment
configuration these operations fail explicitly. Brain and extension Components receive no storage
credentials. R2 requires the same storage integration suite and an explicit data cutover before use.

Ordinary requests leave one global and one per-account admission slot available for provider
downloads, preventing turns from occupying every permit while waiting for their own files.
Transfers keep their permits until completion or cancellation and have a fixed deadline. Bound
the request ceiling and concurrency against the control service's memory budget.

Before restoring account access from a backup, revoke restored attachment capabilities and
reconcile missing objects and later deletions. A database restore must not restore revoked access.
Use a bucket lifecycle longer than the maximum issued TTL as a residual-object cleanup backstop;
Aex's authorization check is the expiry authority.
