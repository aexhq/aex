import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";

const sourceUrl = new URL("../src/otlp-projection.ts", import.meta.url);
const envelopeUrl = new URL("../src/event-envelope.ts", import.meta.url);
const continuationUrl = new URL("../src/continuation-event.ts", import.meta.url);
const internalUrl = new URL("../src/internal.ts", import.meta.url);

describe("public telemetry contract ownership", () => {
  it("owns the projection and OTLP JSON types without importing private platform artifacts", () => {
    expect(existsSync(sourceUrl), "packages/contracts must own src/otlp-projection.ts").toBe(true);
    if (!existsSync(sourceUrl)) return;
    const source = readFileSync(sourceUrl, "utf8");
    expect(source).toMatch(/export\s+(?:interface|type)\s+Otlp(?:Trace|ExportTrace)/);
    expect(source).toMatch(/export\s+(?:interface|type)\s+Otlp(?:Log|ExportLog)/);
    expect(source).not.toMatch(/from\s+["'][^"']*(?:journal|platform|registry)/i);
    expect(source).not.toMatch(/\b(?:Private)?JournalRow\b/);
  });

  it("keeps optional trace identity on the canonical public event envelope", () => {
    const source = readFileSync(envelopeUrl, "utf8");
    expect(source).toMatch(/readonly\s+traceId\?\s*:\s*string/);
    expect(source).toMatch(/readonly\s+spanId\?\s*:\s*string/);
    expect(source).toMatch(/readonly\s+parentSpanId\?\s*:\s*string/);
  });

  it("owns ContinuationEvent traceContext once and exports it only through the internal entrypoint", () => {
    expect(existsSync(continuationUrl), "contracts must own the dependency-free continuation carrier").toBe(true);
    if (!existsSync(continuationUrl)) return;
    const source = readFileSync(continuationUrl, "utf8");
    expect(source).toMatch(/interface\s+ContinuationEvent/);
    expect(source).toMatch(/readonly\s+traceContext\?\s*:\s*(?:W3C)?TraceContext/);
    expect(source).toMatch(/readonly\s+traceparent\s*:\s*string/);
    expect(source).toMatch(/readonly\s+tracestate\?\s*:\s*string/);

    const internal = readFileSync(internalUrl, "utf8");
    expect(internal).toContain('export * from "./continuation-event.js";');
    const publicIndex = readFileSync(fileURLToPath(new URL("../src/index.ts", import.meta.url)), "utf8");
    expect(publicIndex).not.toContain("continuation-event");
  });
});
