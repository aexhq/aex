# Send images and PDFs

Upload a file to Aex and pass the returned media to your agent. Use a model that supports the
file type. PNG, JPEG, GIF, WebP and PDF are supported; your model provider may impose further limits.

After creating a session with the [quickstart](quickstart.md), read and send a local PDF:

```js
import { readFile } from "node:fs/promises";

const bytes = await readFile("report.pdf");
const attachment = await aex.attachments.upload(session.id, bytes, {
  contentType: "application/pdf",
  idempotencyKey: "report-upload-1",
});
await session.send({ message: "Summarize this report.", media: [attachment.media] });
console.log(await session.transcript());
```

Place this before ending the session. Prepaid uploads also need a download allowance and cost
ceiling; see [billing](billing.md#amounts-prices-and-limits). Keep the file available for later
messages that reference it. Delete it with `aex.attachments.delete(session.id, attachment.id)`
when it is no longer needed. Expiry or deletion prevents future reads.

Keep returned URLs private: anyone who has one can fetch the file until access expires or is
revoked. Ending a session keeps its attachments; deleting it revokes them.

## Upload from a desktop without an account key

The trusted backend calls `aex.attachments.grant(sessionId, { content_type, bytes },
{ idempotencyKey })` after authorizing the application's user. Prepaid accounts also supply
`downloadBudgetBytes` and `maxCostMicroUsd`. Capacity is reserved before returning the grant.

Send only that grant to the desktop. It PUTs the exact file bytes to `grant.upload_url` with
`Authorization: Bearer <grant.token>` and the granted `Content-Type`. This bypasses the
application API's file-size and request-time limits. The token authorizes one upload and
cannot read files, submit turns or access account APIs. Repeating identical verified bytes is
safe; a lost storage acknowledgement remains unresolved until cleanup, without another write.

Aex checks actual size and file signature before committing readiness. The backend confirms
completion with `aex.attachments.get(sessionId, grant.id)` and only then receives the media
reference. A desktop's completion assertion never marks a file ready. Grants expire within
ten minutes; failed or expired pending uploads keep their capacity until confirmed deletion.

Call `aex.attachments.limits()` for deployed size, account quota, region, file TTL and session
retention. File expiry cannot exceed the session's remaining retention. Read the actual grant
and attachment expiry; defaults do not promise indefinite storage. Configure exact browser or
WebView origins under `attachments.upload_origins`; upload authentication is still required.

DeepSeek Responses cannot read PDF file inputs. Keep local OCR for the text sent to that model
and retain the original separately. Successful storage does not make a file model-readable.

## Return media from a tool

For Tool-result images, call `return context.finish({ type: "aex_tool_output", version: 1, content, media: [attachment.media] })`
when the Tool has completed. The official loops present that media to the model and preserve the call's source
order even in a parallel batch. Arbitrary JSON/base64 is not implicitly an image. See the
[executable image Tool example](../examples/image-tool.mjs), which publishes through Aex using
the Tool context's session ID. Keep the attachment available for later turns that reference it.

## Lifetime and API behavior

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

## Operator configuration

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

## HTTP reference

| Operation | Authorization and result |
| --- | --- |
| `POST /v1/sessions/{session}/attachments` | Workload key for the owning account; raw bytes, `Content-Type`, required `Idempotency-Key`, optional `x-aex-expires-at` in Unix seconds. Returns `201` with `{ id, media, expires_at }`; completed replay returns `200`. |
| `GET` or `HEAD /v1/attachments/{id}/content?token=…` | The scoped read capability in the URL. Ready, unexpired files require an active account and an owned session. No account Authorization header is needed. |
| `DELETE /v1/sessions/{session}/attachments/{id}` | Owning account key. Revokes new reads immediately and queues deletion; repeat deletion returns `204`. |
