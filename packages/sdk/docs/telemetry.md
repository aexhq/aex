---
title: Telemetry
---

# Telemetry

Events, logs, spans, metrics, traces, and the combined telemetry resource share
one filter model.

```ts
const page = await session.telemetry.query({ limit: 100 });

for await (const frame of session.telemetry.stream({
  origin: { earliest: true }
})) {
  console.log(frame.type);
}
```

Large or long-lived extracts are durable operations:

```ts
const operation = await session.telemetry.export({
  query: {},
  format: "ndjson",
  completeness: "require"
});
const { exportId } = await operation.result();
const grant = await session.telemetry.exports.download(exportId);
```

The download is a short-lived grant. Completeness and gaps remain explicit in
query results and export requests.

The workspace-wide client also exposes the same query resources. To admit
OpenTelemetry payloads directly, use `aex.telemetry.otlp.logs(...)`,
`aex.telemetry.otlp.traces(...)`, or `aex.telemetry.otlp.metrics(...)`.
