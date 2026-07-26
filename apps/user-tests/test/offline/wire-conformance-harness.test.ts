/**
 * C4 — proof that the harness is actually ARMED in the place this suite makes
 * requests, which is a child process and not this one.
 *
 * `04-gates.md`: *"A committed checker that no workflow invokes is worse than no
 * checker — it converts a known gap into false assurance."* The same applies one
 * level down. `installWireConformance` attaches to a module-level observer, and
 * it only sees responses that pass through the `HttpClient` in the SAME module
 * graph. Every scenario in this package runs the SDK in a spawned `bun` child,
 * so an assertion that the harness "is installed" in the test process would
 * prove nothing about whether a single real response is ever checked.
 *
 * This drives the whole path against a real socket, offline, with no
 * credentials: arm an install dir → spawn a child that knows nothing about C4 →
 * the child's responses are observed → a fragment survives its exit → the parent
 * folds it. And it checks the gate FAILS: a bad body and a code-less error body
 * both surface as violations carrying the bytes that caused them.
 */
import { spawn } from "node:child_process";
import { createServer, type Server } from "node:http";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { armWireConformance, readWireConformanceFragments } from "../_fixtures/wire-conformance.js";
import { getBunCommand } from "../_fixtures/install.js";
import type { WireConformanceReport } from "@aexhq/contracts/testing";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..");
const contractsDist = join(repoRoot, "packages", "contracts", "dist");

const TS = "2026-07-25T12:00:00.000Z";
const SESSION = {
  id: "sess_1",
  status: "idle",
  acceptsMessages: true,
  runtimeSize: "0.25cpu-1gb",
  runtimeKind: "lambda",
  createdAt: TS,
  updatedAt: TS,
  dataState: "active"
};

function jsonServer(handler: (path: string) => { status: number; body: unknown }): Promise<{
  readonly origin: string;
  readonly close: () => void;
}> {
  const server: Server = createServer((request, response) => {
    const { status, body } = handler(new URL(request.url ?? "/", "http://127.0.0.1").pathname);
    response.statusCode = status;
    response.setHeader("content-type", "application/json");
    response.end(JSON.stringify(body));
  });
  return new Promise((done) => {
    server.listen(0, "127.0.0.1", () => {
      const address = server.address() as { port: number };
      done({ origin: `http://127.0.0.1:${address.port}`, close: () => server.close() });
    });
  });
}

/** Run a bun script with `cwd = dir`, so the armed `bunfig.toml` applies. */
function runChild(dir: string, script: string): Promise<{ code: number | null; stderr: string }> {
  return new Promise((done) => {
    let stderr = "";
    // `spawn`, never `spawnSync`: this process owns the HTTP server the child
    // talks to, and a synchronous spawn would block the loop that answers it.
    const child = spawn(getBunCommand(), [script], { cwd: dir, stdio: ["ignore", "pipe", "pipe"] });
    child.stderr?.on("data", (chunk) => {
      stderr += String(chunk);
    });
    child.on("close", (code) => done({ code, stderr }));
  });
}

describe("the wire-conformance harness reaches a spawned child", () => {
  let installDir: string;
  let fragmentDir: string;
  let dataPlane: { origin: string; close: () => void };
  let controlPlane: { origin: string; close: () => void };
  let report: WireConformanceReport;
  let childStderr: string;

  beforeAll(async () => {
    dataPlane = await jsonServer((path) => {
      if (path === "/api/sessions/sess_good") return { status: 200, body: { session: SESSION } };
      if (path === "/api/sessions/sess_bad") {
        // An undeclared field on the session: what a strict response object is for.
        return { status: 200, body: { session: { ...SESSION, runtime: { kind: "lambda" } } } };
      }
      if (path === "/api/sessions/sess_missing") {
        return { status: 404, body: { error: "not_found", message: "no such session", requestId: "req_1" } };
      }
      // The shape a rejection produced OUTSIDE a route handler has: no stable
      // code, so it does not satisfy the envelope every operation declares.
      return { status: 403, body: { message: "Forbidden" } };
    });
    // A second origin standing in for the control plane, which serves a
    // DIFFERENT body at the same `/api/whoami` path.
    controlPlane = await jsonServer(() => ({
      status: 200,
      body: { ok: true, principalType: "account_token", appUserId: "u1", scopes: [] }
    }));

    installDir = mkdtempSync(join(tmpdir(), "aex-c4-arm-"));
    fragmentDir = join(installDir, "fragments");
    mkdirSync(fragmentDir, { recursive: true });

    // Stand in for the installed SDK's inlined `dist/_contracts/testing.js`.
    // The child must reach the harness through the SAME module graph its
    // `HttpClient` comes from, which is the whole reason the real fixture
    // resolves it out of the install tree rather than importing the workspace
    // package. Both point into `packages/contracts/dist` here.
    const harnessPath = join(installDir, "harness.mjs");
    writeFileSync(harnessPath, `export * from ${JSON.stringify(join(contractsDist, "testing.js"))};\n`);

    const armed = armWireConformance(installDir, {
      outputDir: fragmentDir,
      origin: dataPlane.origin,
      harnessPath
    });
    expect(armed).toBe(true);

    // A "customer" script: it knows nothing about C4 and is never told.
    const script = join(installDir, "scenario.mjs");
    writeFileSync(
      script,
      `import { HttpClient } from ${JSON.stringify(join(contractsDist, "http.js"))};
const data = new HttpClient({ baseUrl: ${JSON.stringify(dataPlane.origin)}, apiKey: "aex_k" });
const control = new HttpClient({ baseUrl: ${JSON.stringify(controlPlane.origin)}, apiKey: "aex_k" });
await data.request("/api/sessions/sess_good");
await data.request("/api/sessions/sess_bad");
try { await data.request("/api/sessions/sess_missing"); } catch {}
try { await data.request("/api/whoami"); } catch {}
await control.request("/api/whoami");
`
    );

    const child = await runChild(installDir, script);
    childStderr = child.stderr;
    expect(child.code).toBe(0);

    const harvest = readWireConformanceFragments(fragmentDir);
    expect(harvest.armFailures).toEqual([]);
    expect(harvest.reports).toBe(1);
    report = harvest.report;
  });

  afterAll(() => {
    dataPlane?.close();
    controlPlane?.close();
    if (installDir) rmSync(installDir, { recursive: true, force: true });
  });

  it("observes every response the child received, without the child opting in", () => {
    expect(childStderr).not.toContain("could not write");
    expect(report.observed).toBe(5);
    expect(report.originFilter).toBe(dataPlane.origin);
  });

  it("validates a conforming 2xx against its operation's schema", () => {
    expect(report.validated).toContain("sessions.get");
  });

  it("FAILS a 2xx that carries an undeclared field, and quotes the body", () => {
    const violation = report.violations.find((candidate) => candidate.kind === "response");
    expect(violation).toBeDefined();
    expect(violation!.name).toBe("sessions.get");
    expect(violation!.issues.join(" ")).toContain("runtime is not a declared response field");
    expect(violation!.body).toEqual({ session: { ...SESSION, runtime: { kind: "lambda" } } });
  });

  it("checks error envelopes — the responses C4 could not see at all before", () => {
    expect(report.errorsValidated).toContain("GET /api/sessions/sess_missing -> 404");
  });

  it("FAILS an error body with no stable code", () => {
    const violation = report.violations.find((candidate) => candidate.kind === "error-envelope");
    expect(violation).toBeDefined();
    expect(violation!.status).toBe(403);
    expect(violation!.body).toEqual({ message: "Forbidden" });
  });

  it("does not judge the other plane's body by this plane's schema", () => {
    // Without the origin, `GET /api/whoami` from the control plane matches the
    // data-plane binding and reports a violation that is not one.
    expect(report.offPlane).toEqual([`${controlPlane.origin} GET /api/whoami`]);
    expect(report.violations.some((candidate) => candidate.origin === controlPlane.origin)).toBe(false);
  });

  it("reports exactly the two real violations and no others", () => {
    expect(report.violations).toHaveLength(2);
  });
});
