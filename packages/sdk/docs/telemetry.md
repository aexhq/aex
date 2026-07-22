---
title: OpenTelemetry export
---

# OpenTelemetry export

aex exposes each session's customer-visible trace and log timeline as
standards-pure OTLP/HTTP JSON. The session journal remains the source of truth;
the API derives telemetry from that durable history when you read it. This is
a pull surface. It does not configure a push drain or create another telemetry
store.

## SDK

Open a session and iterate its trace or log pages:

```ts
import { Aex } from "aex";

const aex = new Aex({ apiKey: process.env.AEX_API_KEY! });
const session = await aex.sessions.open("ses_...");

for await (const request of session.otel.traces()) {
  // `request` is an OTLP ExportTraceServiceRequest JSON body.
  await fetch("http://localhost:4318/v1/traces", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(request)
  });
}

for await (const request of session.otel.logs()) {
  // `request` is an OTLP ExportLogsServiceRequest JSON body.
}
```

The SDK follows the API's opaque `x-aex-next-cursor` response header. Cursor
metadata is never inserted into the OTLP body. Each yielded value can therefore
be posted directly to the corresponding OTLP/HTTP endpoint.

## CLI

Export all trace pages as one OTLP body:

```bash
aex otel <session-id> --signal traces --json > traces.otlp.json
curl -H 'content-type: application/json' \
  --data-binary @traces.otlp.json \
  http://localhost:4318/v1/traces
```

Use `--signal logs` for an `ExportLogsServiceRequest`. Without `--json`, the
same body is printed with indentation for reading. A local OpenTelemetry
Collector can forward these requests to Jaeger or another compatible backend.

## Data boundary

Only customer-visible, public-safe attributes are projected. Arbitrary event
payload fields, prompts, credentials, private provider metadata, URLs, and
operational details do not become telemetry attributes. Trace and span IDs on
the event envelope let this external projection correlate with the same turn
without exposing a private journal contract.

The OpenTelemetry `gen_ai.*` semantic conventions are pre-1.0 and may evolve.
aex keeps its documented public projection stable, while a future release may
adopt renamed upstream attributes through the normal versioned release process.
