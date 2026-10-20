# Send images and PDFs

Upload a file to an owned session and send its returned media reference. Aex stores immutable files in private S3-compatible storage. It converts PDFs into bounded page images using an isolated Poppler process; Pi, Codex and OpenCode receive image bytes through their native adapters. Choose a model with image support.

```js
import { readFile } from "node:fs/promises";
const attachment = await aex.attachments.upload(session.id, await readFile("report.pdf"), {
  contentType: "application/pdf", idempotencyKey: "report-upload-1",
  downloadBudgetBytes: 10485760, maxCostMicroUsd: 1000000,
});
console.log((await session.send({ text: "Summarize this report", media: [attachment.media] })).output);
```

PNG, JPEG, GIF, WebP and PDF uploads have signature, size, account-quota and expiry checks. `aex.attachments.limits()` returns deployed limits. Prepaid uploads reserve storage and a download allowance before storage effects. Reuse the same key and bytes after a lost acknowledgement; uncertain writes are reconciled before another effect.

Keep read URLs private. Anyone holding one can fetch it while its account, session and attachment remain active. Another tenant cannot use it as that tenant's model input. `aex.attachments.delete(session.id,attachment.id)` revokes future reads. Session close keeps files; session deletion revokes files and transient PDF pages and waits for confirmed object cleanup. Native checkpoints retain already consumed image context until session deletion.

## Direct client uploads

A trusted backend can issue `aex.attachments.grant(sessionId,{content_type,bytes},{idempotencyKey,downloadBudgetBytes,maxCostMicroUsd})` after authorizing its application user. Send only that grant to a desktop or browser. PUT the exact bytes to `grant.upload_url`, using its bearer token and declared content type. The token grants one upload and no model, account or read authority.

The backend calls `aex.attachments.get(sessionId,grant.id)` to confirm readiness and obtain the media reference. A client assertion never marks a file ready. Grants expire within ten minutes; failed uploads retain capacity until cleanup is confirmed. Configure exact upload origins for browser CORS.

External URLs, inline base64 and the retired `aex_tool_output` envelope are not attachment inputs. Send owned media through `session.send({text,media})`.
