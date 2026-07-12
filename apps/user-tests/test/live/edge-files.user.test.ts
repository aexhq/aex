/**
 * Live edge-case sweep: SessionFiles (files & downloads surface).
 *
 * Acts as a real customer hammering the FILES + DOWNLOAD verbs of the
 * installed `@aexhq/sdk` against the DEV plane, hunting for edge-case defects
 * before a prod launch. The existing live tests exercise `list()` + the archive
 * `download()` verbs; NONE exercise `read` / `find` / `findOne` / `link` /
 * `fetch` or the file selector matrix through the session accessor. This file
 * closes that gap.
 *
 * Surface under test (packages/sdk/src/client.ts `SessionFiles`):
 *   list / last / first / read(selector) / find(query) / findOne(query) /
 *   link(selectorOrQuery) / fetch(selectorOrQuery) / download(selector?)
 *   + session.download() / session.downloadMetadata()
 *
 * Model: deepseek-v4-flash, BYOK via the gate-provider apiKeys map. Tiny prompts. Four
 * live sessions total (A rich-selector-matrix, B large-file round-trip, C
 * unicode+space filename, D no-files), each independent, each probing many
 * facets in ONE child process and emitting a JSON verdict the parent asserts on.
 *
 * Required env: AEX_API_URL, AEX_API_KEY, DEEPSEEK_API_KEY, +
 * AEX_USER_TEST_TARBALL/VERSION (wired by the shared runner).
 */
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getBunCommand, installAex, runCommand, type InstallResult } from "../_fixtures/install.js";
import { GATE_PROVIDER, gateModel, requireGateKey } from "../_fixtures/provider.js";

function requireEnv(name: string): string {
  const value = process.env[name];
  if (!value || value.length === 0) {
    throw new Error(`user-tests live (edge-files): required env ${name} is missing.`);
  }
  return value;
}

const apiUrl = requireEnv("AEX_API_URL");
const apiKey = requireEnv("AEX_API_KEY");
const providerKey = requireGateKey("edge-files");
const model = gateModel();

function buildPassEnv(extras: Record<string, string>): Record<string, string> {
  const env: Record<string, string> = { ...extras };
  const pathKey = process.platform === "win32" ? "Path" : "PATH";
  if (process.env[pathKey]) env[pathKey] = process.env[pathKey]!;
  if (process.platform === "win32") {
    for (const k of [
      "SystemRoot",
      "SystemDrive",
      "TEMP",
      "TMP",
      "USERPROFILE",
      "APPDATA",
      "LOCALAPPDATA",
      "ComSpec",
      "ProgramFiles",
      "ProgramData"
    ]) {
      if (process.env[k]) env[k] = process.env[k]!;
    }
  } else {
    for (const k of ["HOME", "TMPDIR", "LANG", "LC_ALL"]) {
      if (process.env[k]) env[k] = process.env[k]!;
    }
  }
  return env;
}

/** Small helpers injected into every child script. */
const CHILD_PRELUDE = `
  import { Aex } from "@aexhq/sdk";
  import { strFromU8, unzipSync } from "fflate";
  const HTTP_DEBUG_LINES = [];
  const pushHttpDebug = (line) => {
    HTTP_DEBUG_LINES.push(String(line).slice(0, 500));
    if (HTTP_DEBUG_LINES.length > 80) HTTP_DEBUG_LINES.shift();
  };
  const redactedUrlForDebug = (input) => {
    const raw = typeof input === "string" || input instanceof URL ? String(input) : (input && typeof input.url === "string" ? input.url : "");
    try {
      const u = new URL(raw);
      return u.origin + u.pathname;
    } catch {
      return "[non-url]";
    }
  };
  const redactTextForDebug = (text) => String(text).replace(/https?:\\/\\/[^\\s<>"'\`]+/g, (raw) => {
    try {
      const u = new URL(raw);
      return u.origin + u.pathname + (u.search ? "?[redacted]" : "");
    } catch {
      return "[redacted-url]";
    }
  });
  const tracedFetch = async (input, init) => {
    const started = Date.now();
    const method = (init && init.method) || (input && typeof input.method === "string" ? input.method : "GET");
    const url = redactedUrlForDebug(input);
    try {
      const res = await fetch(input, init);
      const finalUrl = res.url ? " final=" + redactedUrlForDebug(res.url) : "";
      pushHttpDebug("[fetch] " + method + " " + url + " -> " + res.status + " " + (Date.now() - started) + "ms" + finalUrl);
      return res;
    } catch (error) {
      const name = error && error.constructor ? error.constructor.name : "Error";
      const message = redactTextForDebug(error && error.message ? String(error.message).slice(0, 180) : String(error));
      pushHttpDebug("[fetch] " + method + " " + url + " !! " + name + ": " + message + " " + (Date.now() - started) + "ms");
      throw error;
    }
  };
  const client = new Aex({
    baseUrl: process.env.AEX_API_URL,
    apiKey: process.env.AEX_API_KEY,
    fetch: tracedFetch,
    debug: (line) => pushHttpDebug("[sdk] " + line)
  });
  const debugTail = () => HTTP_DEBUG_LINES.slice(-80);
  const PROVIDER = process.env.PROVIDER;
const PROVIDER_KEY = process.env.PROVIDER_KEY;
  const MODEL = process.env.MODEL;

  const PROBE_TIMEOUT_MS = 45000;
  const errorStringField = (error, key) => {
    const value = error && error[key];
    return typeof value === "string" ? redactTextForDebug(value) : null;
  };
  const errorNumberField = (error, key) => {
    const value = error && error[key];
    return typeof value === "number" && Number.isFinite(value) ? value : null;
  };
  const errorCauseCode = (error) => {
    const direct = errorStringField(error, "causeCode");
    if (direct) return direct;
    const cause = error && error.cause;
    return cause && typeof cause.code === "string" ? cause.code : null;
  };
  // Wrap a probe so ONE failing/hanging verb never aborts the whole script:
  // record a structured {label, ok, value|error}. The SDK has its own bounded
  // The SDK owns transfer retries; this outer race only bounds the test process.
  async function probe(label, fn) {
    try {
      const value = await Promise.race([
        Promise.resolve().then(fn),
        new Promise((_, rej) => setTimeout(() => rej(new Error("PROBE_TIMEOUT_45S")), PROBE_TIMEOUT_MS))
      ]);
      return { label, ok: true, value };
    } catch (e) {
      return {
        label,
        ok: false,
        error: {
          name: e && e.constructor ? e.constructor.name : "Error",
          message: redactTextForDebug(e && e.message ? String(e.message) : String(e)),
          status: e && typeof e.status === "number" ? e.status : null,
          code: e && typeof e.code === "string" ? e.code : null,
          causeCode: errorCauseCode(e),
          attempts: errorNumberField(e, "attempts"),
          elapsedMs: errorNumberField(e, "elapsedMs"),
          method: errorStringField(e, "method"),
          host: errorStringField(e, "host"),
          path: errorStringField(e, "path")
        }
      };
    }
  }
  const zipProbe = (bytes) => {
    const magicOk = !!bytes && bytes.byteLength >= 4 && bytes[0] === 0x50 && bytes[1] === 0x4b && bytes[2] === 0x03 && bytes[3] === 0x04;
    const entries = magicOk ? unzipSync(bytes) : {};
    const entryNames = Object.keys(entries).sort();
    const manifestBytes = entries["manifest.json"];
    const manifest = manifestBytes ? JSON.parse(strFromU8(manifestBytes)) : null;
    const errors = Array.isArray(manifest?.errors) ? manifest.errors : [];
    return {
      byteLength: bytes ? bytes.byteLength : 0,
      magicOk,
      entries: entryNames,
      hasManifest: !!manifest,
      manifestErrors: errors.map((error) => ({
        namespace: error.namespace ?? null,
        id: error.id ?? null,
        filename: error.filename ?? null,
        message: String(error.message ?? "").slice(0, 300)
      }))
    };
  };
  function zipProbeNoTransientManifestErrors(bytes) {
    const result = zipProbe(bytes);
    const transientErrors = result.manifestErrors.filter((error) =>
      transientProbeRe.test(String(error.message ?? ""))
    );
    if (transientErrors.length > 0) {
      const error = new Error(
        "zip manifest recorded transient per-artifact download errors: " +
          JSON.stringify(transientErrors).slice(0, 500)
      );
      error.code = "NETWORK_ERROR";
      throw error;
    }
    return result;
  }
  const dec = (bytes) => new TextDecoder().decode(bytes);
`;

async function runChild(
  install: InstallResult,
  scriptName: string,
  body: string,
  timeoutMs = 8 * 60_000
): Promise<Record<string, unknown>> {
  const scriptPath = join(install.installDir, scriptName);
  writeFileSync(scriptPath, `${CHILD_PRELUDE}\n${body}\n`);
  const child = await runCommand(getBunCommand(), [scriptPath], {
    cwd: install.installDir,
    timeoutMs,
    env: buildPassEnv({
      AEX_API_URL: apiUrl,
      AEX_API_KEY: apiKey,
      PROVIDER: GATE_PROVIDER, PROVIDER_KEY: providerKey,
      MODEL: model
    })
  });
  if (child.exitCode !== 0) {
    throw new Error(
      `edge-files runner (${scriptName}) exited ${child.exitCode}:\n--- stdout ---\n${child.stdout}\n--- stderr ---\n${child.stderr}`
    );
  }
  try {
    return JSON.parse(child.stdout.trim()) as Record<string, unknown>;
  } catch {
    throw new Error(`edge-files runner (${scriptName}) produced non-JSON stdout:\n${child.stdout}`);
  }
}

type ProbeResult = {
  label: string;
  ok: boolean;
  value?: unknown;
  error?: {
    name: string;
    message: string;
    status: number | null;
    code: string | null;
    causeCode?: string | null;
    attempts?: number | null;
    elapsedMs?: number | null;
    method?: string | null;
    host?: string | null;
    path?: string | null;
  };
};
function byLabel(probes: ProbeResult[], label: string): ProbeResult {
  const p = probes.find((x) => x.label === label);
  if (!p) throw new Error(`probe "${label}" missing from child result; got: ${probes.map((x) => x.label).join(", ")}`);
  return p;
}

let install: InstallResult;
beforeAll(async () => {
  install = await installAex();
}, 240_000);
afterAll(() => {
  install?.cleanup();
});

describe("edge: SessionFiles read/find/link/fetch/download selector matrix", () => {
  it(
    "A: small deliverable — every read/find/link/fetch/download selector resolves; bad selectors error cleanly",
    async () => {
      const marker = "MK" + Math.random().toString(36).slice(2, 10).toUpperCase();
      const prompt =
        `Use your shell/filesystem tools to create a text file at the path ` +
        `/workspace/files/report.txt whose ENTIRE contents are exactly these characters: ${marker} ` +
        `(no trailing newline, nothing else). Do not create any other files. Then reply with the single word done.`;
      const body = `
        const sessionResult = await client.start({
          provider: PROVIDER,
          model: MODEL,
          message: ${JSON.stringify(prompt)},
          builtinTools: "default",
          fileCapture: { allowedDirs: ["/workspace/files"] },
          apiKeys: { [PROVIDER]: PROVIDER_KEY },
          idempotencyKey: "edge-files-A-" + Date.now()
        }, { timeoutMs: 6 * 60_000 });
        const sessionId = sessionResult.sessionId;
        const status = sessionResult.ok ? "succeeded" : (sessionResult.status || "failed");
        const session = await client.sessions.open(sessionId);
        const outs = session.files;

        // list (sessions endpoint) and find({}) (sessions endpoint) — cross-check parity.
        const listed = (await outs.list()).files;
        const listMeta = listed.map((o) => ({ id: o.id, filename: o.filename ?? null, sizeBytes: o.sizeBytes ?? null, contentType: o.contentType ?? null }));
        const found = await outs.find({});
        const findMeta = found.map((o) => ({ id: o.id, filename: o.filename ?? null }));

        const report = listed.find((o) => (o.filename || "").endsWith("report.txt")) || null;
        const reportIdx = report ? listed.indexOf(report) : -1;
        const exactPath = report ? report.filename : "report.txt";

        const probes = [];
        probes.push(await probe("read_suffix", async () => await outs.read({ path: "report.txt", match: "suffix" })));
        probes.push(await probe("read_exact", async () => await outs.read({ path: exactPath })));
        probes.push(await probe("read_file_obj", async () => report ? await outs.read(report) : null));
        probes.push(await probe("read_by_id", async () => report ? await outs.read({ id: report.id }) : null));
        const LIVE_FILE_TRANSFER_TIMEOUT_MS = 20_000;
        probes.push(await probe("read_timeout_option", async () => report ? await outs.read(report, { timeoutMs: LIVE_FILE_TRANSFER_TIMEOUT_MS }) : null));
        probes.push(await probe("find_regex", async () => (await outs.find({ filename: /report\\.txt$/ })).length));
        probes.push(await probe("find_extension", async () => (await outs.find({ extension: "txt" })).length));
        probes.push(await probe("find_type_text", async () => (await outs.find({ type: "text" })).length));
        probes.push(await probe("findOne_match", async () => { const o = await outs.findOne({ filename: "report.txt" }); return o ? { id: o.id, filename: o.filename ?? null } : null; }));
        probes.push(await probe("findOne_nomatch_null", async () => await outs.findOne({ filename: "does-not-exist-xyz.txt" })));
        probes.push(await probe("last", async () => { const o = await outs.last(); return o ? (o.filename ?? o.id) : null; }));
        probes.push(await probe("first", async () => { const o = await outs.first(); return o ? (o.filename ?? o.id) : null; }));

        // link + fetch a presigned URL, then GET it with global fetch.
        probes.push(await probe("link", async () => {
          const link = await outs.link({ filename: "report.txt" });
          const resp = await tracedFetch(link.url);
          const getStatus = resp.status;
          const getText = (await resp.text()).slice(0, 256);
          return { hasUrl: typeof link.url === "string" && link.url.length > 0, expiresInSeconds: link.expiresInSeconds ?? null, getStatus, getText };
        }));
        probes.push(await probe("fetch", async () => {
          const resp = await outs.fetch({ filename: "report.txt" });
          return { status: resp.status, text: (await resp.text()).slice(0, 256) };
        }));

        // download one file's raw bytes (by SessionFile selector).
        probes.push(await probe("download_selector", async () => {
          if (!report) return null;
          const bytes = await outs.download(report);
          return { len: bytes.byteLength, text: dec(bytes).slice(0, 256) };
        }));
        probes.push(await probe("download_selector_timeout_option", async () => {
          if (!report) return null;
          const bytes = await outs.download(report, { timeoutMs: LIVE_FILE_TRANSFER_TIMEOUT_MS });
          return { len: bytes.byteLength, text: dec(bytes).slice(0, 256) };
        }));
        // archive verbs.
        probes.push(await probe("download_files_zip", async () => zipProbeNoTransientManifestErrors(await outs.download(undefined))));
        probes.push(await probe("download_files_zip_timeout_option", async () => zipProbeNoTransientManifestErrors(await outs.download(undefined, { timeoutMs: LIVE_FILE_TRANSFER_TIMEOUT_MS }))));
        probes.push(await probe("download_all_zip", async () => zipProbeNoTransientManifestErrors(await session.download())));
        probes.push(await probe("download_metadata_zip", async () => zipProbe(await session.downloadMetadata())));

        // Bad-selector / boundary probes — must error CLEANLY (no hang).
        probes.push(await probe("read_missing_path", async () => await outs.read({ path: "nope-" + Date.now() + ".txt", match: "suffix" })));
        probes.push(await probe("download_missing_id", async () => await outs.download({ id: "file_nonexistent_zzz" })));
        probes.push(await probe("link_nomatch", async () => await outs.link({ filename: "nope-" + Date.now() + ".txt" })));
        probes.push(await probe("link_expires_zero", async () => await outs.link({ filename: "report.txt" }, { expiresIn: 0 })));
        probes.push(await probe("link_expires_badpreset", async () => await outs.link({ filename: "report.txt" }, { expiresIn: "5m" })));

        process.stdout.write(JSON.stringify({ sessionId, status, marker: ${JSON.stringify(marker)}, listMeta, findMeta, reportIdx, exactPath, httpDebug: debugTail(), probes }));
        process.exit(0);
      `;
      const r = await runChild(install, "edge-files-A.mjs", body, 9 * 60_000);
      const ctxPayload = {
        httpDebug: r.httpDebug,
        sessionId: r.sessionId,
        status: r.status,
        marker: r.marker,
        listMeta: r.listMeta,
        findMeta: r.findMeta,
        reportIdx: r.reportIdx,
        exactPath: r.exactPath,
        probes: r.probes
      };
      const ctx = `\n\n${JSON.stringify(ctxPayload, null, 2).slice(0, 6000)}`;
      const probes = r.probes as ProbeResult[];
      const listMeta = r.listMeta as Array<{ id: string; filename: string | null; sizeBytes: number | null }>;
      const findMeta = r.findMeta as Array<{ id: string; filename: string | null }>;

      expect(r.status, `run did not succeed${ctx}`).toBe("succeeded");

      // 1. list() returns the deliverable with a filename + positive sizeBytes.
      const report = listMeta.find((o) => (o.filename || "").endsWith("report.txt"));
      expect(report, `report.txt missing from list()${ctx}`).toBeTruthy();
      expect(report!.sizeBytes, `report.txt sizeBytes not positive${ctx}`).toBeGreaterThan(0);

      // 2. No internal/diagnostic namespace bleeds into the deliverables listing.
      const leaked = listMeta.filter((o) => o.filename && (o.filename.startsWith("runtime/") || o.filename.startsWith("host/")));
      expect(leaked, `diagnostic namespace leaked into files list${ctx}`).toEqual([]);

      // 3. list() (sessions endpoint) and find({}) (sessions endpoint) agree —
      //    the two endpoints must not diverge for the same deliverable set.
      expect(new Set(findMeta.map((o) => o.id)), `list()/find({}) id-set divergence${ctx}`)
        .toEqual(new Set(listMeta.map((o) => o.id)));

      // 4. read via every selector shape returns the exact content.
      for (const label of ["read_suffix", "read_exact", "read_file_obj", "read_by_id", "read_timeout_option"]) {
        const p = byLabel(probes, label);
        expect(p.ok, `${label} threw: ${JSON.stringify(p.error)}${ctx}`).toBe(true);
        const v = p.value as { text: string; truncated: boolean; totalBytes: number; file?: { sizeBytes?: number } };
        expect(v.text, `${label} wrong text${ctx}`).toBe(r.marker);
        expect(v.truncated, `${label} unexpectedly truncated${ctx}`).toBe(false);
        // totalBytes must equal the byte size of the marker (ASCII → 1 byte/char).
        expect(v.totalBytes, `${label} totalBytes != marker length${ctx}`).toBe((r.marker as string).length);
      }

      // 5. find / findOne semantics.
      expect((byLabel(probes, "find_regex").value as number), `find regex 0 hits${ctx}`).toBeGreaterThanOrEqual(1);
      expect((byLabel(probes, "find_extension").value as number), `find extension txt 0 hits${ctx}`).toBeGreaterThanOrEqual(1);
      expect((byLabel(probes, "find_type_text").value as number), `find type text 0 hits${ctx}`).toBeGreaterThanOrEqual(1);
      const findOneMatch = byLabel(probes, "findOne_match").value as { id: string } | null;
      expect(findOneMatch, `findOne match returned null${ctx}`).toBeTruthy();
      const findOneNull = byLabel(probes, "findOne_nomatch_null");
      expect(findOneNull.ok && findOneNull.value === null, `findOne no-match must return null, not throw${ctx}`).toBe(true);

      // 6. link → usable presigned URL that GETs 200 with the content;
      //    fetch() → Response with the bytes.
      const link = byLabel(probes, "link");
      expect(link.ok, `link threw: ${JSON.stringify(link.error)}${ctx}`).toBe(true);
      const lv = link.value as { hasUrl: boolean; getStatus: number | null; getText: string | null };
      expect(lv.hasUrl, `link returned no url${ctx}`).toBe(true);
      expect(lv.getStatus, `presigned URL GET not 200${ctx}`).toBe(200);
      expect(lv.getText, `presigned URL body mismatch${ctx}`).toBe(r.marker);
      const fetchP = byLabel(probes, "fetch");
      expect(fetchP.ok, `fetch threw: ${JSON.stringify(fetchP.error)}${ctx}`).toBe(true);
      const fv = fetchP.value as { status: number; text: string };
      expect(fv.status, `fetch() status not 200${ctx}`).toBe(200);
      expect(fv.text, `fetch() body mismatch${ctx}`).toBe(r.marker);

      // 7. download(selector) → raw bytes == content; archive verbs → valid zips.
      const dsel = byLabel(probes, "download_selector");
      expect(dsel.ok, `download(selector) threw: ${JSON.stringify(dsel.error)}${ctx}`).toBe(true);
      const dv = dsel.value as { len: number; text: string };
      expect(dv.text, `download(selector) content mismatch${ctx}`).toBe(r.marker);
      expect(dv.len, `download(selector) len != sizeBytes${ctx}`).toBe(report!.sizeBytes);
      const dselTimeout = byLabel(probes, "download_selector_timeout_option");
      expect(dselTimeout.ok, `download(selector, timeoutMs) threw: ${JSON.stringify(dselTimeout.error)}${ctx}`).toBe(true);
      const dtv = dselTimeout.value as { len: number; text: string };
      expect(dtv.text, `download(selector, timeoutMs) content mismatch${ctx}`).toBe(r.marker);
      expect(dtv.len, `download(selector, timeoutMs) len != sizeBytes${ctx}`).toBe(report!.sizeBytes);
      for (const label of ["download_files_zip", "download_files_zip_timeout_option", "download_all_zip", "download_metadata_zip"]) {
        const p = byLabel(probes, label);
        expect(p.ok, `${label} threw: ${JSON.stringify(p.error)}${ctx}`).toBe(true);
        const z = p.value as { byteLength: number; magicOk: boolean; hasManifest: boolean; manifestErrors: unknown[] };
        expect(z.byteLength, `${label} empty${ctx}`).toBeGreaterThan(0);
        expect(z.magicOk, `${label} not a valid zip${ctx}`).toBe(true);
        expect(z.hasManifest, `${label} missing manifest.json${ctx}`).toBe(true);
        expect(z.manifestErrors, `${label} manifest recorded per-artifact download errors${ctx}`).toEqual([]);
      }

      // 8. Bad selectors error CLEANLY (a real error, not a hang/PROBE_TIMEOUT).
      for (const label of ["read_missing_path", "download_missing_id", "link_nomatch", "link_expires_zero", "link_expires_badpreset"]) {
        const p = byLabel(probes, label);
        expect(p.ok, `${label} should have thrown but resolved to ${JSON.stringify(p.value)}${ctx}`).toBe(false);
        expect(p.error!.message, `${label} hung instead of erroring${ctx}`).not.toContain("PROBE_TIMEOUT");
        expect((p.error!.message || "").length, `${label} error message empty${ctx}`).toBeGreaterThan(0);
      }
    },
    10 * 60_000
  );

  it(
    "B: large (~60KB) file — bytes round-trip; read() caps at maxBytes with correct truncated/totalBytes",
    async () => {
      const prompt =
        `Create a text file at /workspace/files/big.txt containing the single letter A repeated exactly 60000 times ` +
        `(60000 bytes, no newline, nothing else). Generate it precisely with a shell command, for example: ` +
        `python3 -c "open('/workspace/files/big.txt','w').write('A'*60000)". Then reply with the single word done.`;
      const body = `
        const sessionResult = await client.start({
          provider: PROVIDER,
          model: MODEL,
          message: ${JSON.stringify(prompt)},
          builtinTools: "default",
          fileCapture: { allowedDirs: ["/workspace/files"] },
          apiKeys: { [PROVIDER]: PROVIDER_KEY },
          idempotencyKey: "edge-files-B-" + Date.now()
        }, { timeoutMs: 6 * 60_000 });
        const sessionId = sessionResult.sessionId;
        const status = sessionResult.ok ? "succeeded" : (sessionResult.status || "failed");
        const session = await client.sessions.open(sessionId);
        const outs = session.files;
        const listed = (await outs.list()).files;
        const big = listed.find((o) => (o.filename || "").endsWith("big.txt")) || null;
        const sizeBytes = big ? (big.sizeBytes ?? null) : null;

        const probes = [];
        probes.push(await probe("download_full", async () => {
          if (!big) return null;
          const bytes = await outs.download(big);
          const text = dec(bytes);
          return { len: bytes.byteLength, allA: /^A+$/.test(text), first: text.slice(0, 4), last: text.slice(-4) };
        }));
        probes.push(await probe("read_default_cap", async () => {
          if (!big) return null;
          const t = await outs.read(big);
          return { textLen: t.text.length, truncated: t.truncated, totalBytes: t.totalBytes };
        }));
        probes.push(await probe("read_raised_cap", async () => {
          if (!big || sizeBytes == null) return null;
          const t = await outs.read(big, { maxBytes: sizeBytes + 5000 });
          return { textLen: t.text.length, truncated: t.truncated, totalBytes: t.totalBytes, allA: /^A+$/.test(t.text) };
        }));

        process.stdout.write(JSON.stringify({ sessionId, status, sizeBytes, filename: big ? big.filename : null, probes }));
        process.exit(0);
      `;
      const r = await runChild(install, "edge-files-B.mjs", body, 9 * 60_000);
      const ctx = `\n\n${JSON.stringify(r, null, 2).slice(0, 3000)}`;
      const probes = r.probes as ProbeResult[];
      const sizeBytes = r.sizeBytes as number | null;

      expect(r.status, `run did not succeed${ctx}`).toBe("succeeded");
      expect(sizeBytes, `big.txt missing / no sizeBytes${ctx}`).toBeTruthy();
      // The truncation edge case is only meaningful above the 50_000 default cap.
      expect(sizeBytes!, `model produced a file <= 50KB, cannot exercise truncation (model variance)${ctx}`).toBeGreaterThan(50_000);

      const full = byLabel(probes, "download_full");
      expect(full.ok, `download_full threw: ${JSON.stringify(full.error)}${ctx}`).toBe(true);
      const fv = full.value as { len: number; allA: boolean };
      expect(fv.len, `downloaded length != sizeBytes (truncation/corruption)${ctx}`).toBe(sizeBytes);
      expect(fv.allA, `downloaded content not all 'A' (corruption)${ctx}`).toBe(true);

      const def = byLabel(probes, "read_default_cap");
      expect(def.ok, `read_default_cap threw: ${JSON.stringify(def.error)}${ctx}`).toBe(true);
      const dv = def.value as { textLen: number; truncated: boolean; totalBytes: number };
      expect(dv.truncated, `read() over the 50KB default cap must set truncated=true${ctx}`).toBe(true);
      expect(dv.textLen, `read() default cap text length must be 50_000${ctx}`).toBe(50_000);
      expect(dv.totalBytes, `read() default cap totalBytes must equal full size${ctx}`).toBe(sizeBytes);

      const raised = byLabel(probes, "read_raised_cap");
      expect(raised.ok, `read_raised_cap threw: ${JSON.stringify(raised.error)}${ctx}`).toBe(true);
      const rv = raised.value as { textLen: number; truncated: boolean; totalBytes: number; allA: boolean };
      expect(rv.truncated, `read() above file size must NOT be truncated${ctx}`).toBe(false);
      expect(rv.textLen, `read() raised cap must return the full text${ctx}`).toBe(sizeBytes);
      expect(rv.totalBytes, `read() raised cap totalBytes${ctx}`).toBe(sizeBytes);
      expect(rv.allA, `read() raised cap content not all 'A'${ctx}`).toBe(true);
    },
    10 * 60_000
  );

  it(
    "C: unicode + space in session filename — listing, read, link/fetch all preserve it",
    async () => {
      const marker = "CAFE" + Math.random().toString(36).slice(2, 8).toUpperCase();
      // Filename with a non-ASCII char (é) AND a space.
      const command =
        "mkdir -p /workspace/files && printf '%s' '" + marker + "' > '/workspace/files/café menu.txt'";
      const prompt =
        `Use the shell tool once. The complete shell command is between the fences; do not add prose or tokens to it.\n` +
        "```bash\n" +
        command +
        "\n```\n" +
        `After the command succeeds, reply with exactly: done`;
      const body = `
        const sessionResult = await client.start({
          provider: PROVIDER,
          model: MODEL,
          message: ${JSON.stringify(prompt)},
          builtinTools: "default",
          fileCapture: { allowedDirs: ["/workspace/files"] },
          apiKeys: { [PROVIDER]: PROVIDER_KEY },
          idempotencyKey: "edge-files-C-" + Date.now()
        }, { timeoutMs: 6 * 60_000 });
        const sessionId = sessionResult.sessionId;
        const status = sessionResult.ok ? "succeeded" : (sessionResult.status || "failed");
        const session = await client.sessions.open(sessionId);
        const outs = session.files;
        const listed = (await outs.list()).files;
        const listNames = listed.map((o) => o.filename ?? null);
        const target = listed.find((o) => (o.filename || "").endsWith("menu.txt")) || null;

        const probes = [];
        probes.push(await probe("read_suffix_unicode", async () => target ? (await outs.read({ path: "café menu.txt", match: "suffix" })).text : null));
        probes.push(await probe("read_by_id", async () => target ? (await outs.read({ id: target.id })).text : null));
        probes.push(await probe("download_by_id", async () => { if (!target) return null; const b = await outs.download(target); return { len: b.byteLength, text: dec(b) }; }));
        probes.push(await probe("link_fetch_unicode", async () => {
          if (!target) return null;
          const link = await outs.link({ id: target.id });
          const resp = await tracedFetch(link.url);
          return { status: resp.status, text: (await resp.text()) };
        }));

        process.stdout.write(JSON.stringify({ sessionId, status, marker: ${JSON.stringify(marker)}, listNames, targetFilename: target ? target.filename : null, probes }));
        process.exit(0);
      `;
      const r = await runChild(install, "edge-files-C.mjs", body, 9 * 60_000);
      const ctx = `\n\n${JSON.stringify(r, null, 2).slice(0, 3000)}`;
      const probes = r.probes as ProbeResult[];
      const targetFilename = r.targetFilename as string | null;

      expect(r.status, `run did not succeed${ctx}`).toBe("succeeded");
      expect(targetFilename, `unicode file not found in listing (model may not have created it)${ctx}`).toBeTruthy();
      // The é (U+00E9) and the space must survive capture → storage → SDK JSON.
      expect(targetFilename!.includes("é"), `non-ASCII 'é' lost from filename: ${JSON.stringify(targetFilename)}${ctx}`).toBe(true);
      expect(targetFilename!.includes(" "), `space lost from filename: ${JSON.stringify(targetFilename)}${ctx}`).toBe(true);

      const rs = byLabel(probes, "read_suffix_unicode");
      expect(rs.ok, `read by unicode suffix threw: ${JSON.stringify(rs.error)}${ctx}`).toBe(true);
      expect(rs.value, `read by unicode suffix wrong content${ctx}`).toBe(r.marker);
      const rid = byLabel(probes, "read_by_id");
      expect(rid.ok && rid.value === r.marker, `read by id wrong content${ctx}`).toBe(true);
      const dl = byLabel(probes, "download_by_id");
      expect(dl.ok, `download unicode file threw: ${JSON.stringify(dl.error)}${ctx}`).toBe(true);
      expect((dl.value as { text: string }).text, `download unicode content mismatch${ctx}`).toBe(r.marker);
      const lf = byLabel(probes, "link_fetch_unicode");
      expect(lf.ok, `link/fetch unicode file threw: ${JSON.stringify(lf.error)}${ctx}`).toBe(true);
      const lv = lf.value as { status: number; text: string };
      expect(lv.status, `presigned GET for unicode filename not 200${ctx}`).toBe(200);
      expect(lv.text, `presigned GET body mismatch for unicode filename${ctx}`).toBe(r.marker);
    },
    10 * 60_000
  );

  it(
    "D: session with NO files — list() is empty, bad reads error, archive verbs still yield valid zips",
    async () => {
      const probe = "NOFILES-" + Math.random().toString(36).slice(2, 8);
      const prompt = `SessionFile verbatim: ${probe}. Do not create, write, or save any files.`;
      const body = `
        const sessionResult = await client.start({
          provider: PROVIDER,
          model: MODEL,
          message: ${JSON.stringify(prompt)},
          apiKeys: { [PROVIDER]: PROVIDER_KEY },
          idempotencyKey: "edge-files-D-" + Date.now()
        }, { timeoutMs: 6 * 60_000 });
        const sessionId = sessionResult.sessionId;
        const status = sessionResult.ok ? "succeeded" : (sessionResult.status || "failed");
        const session = await client.sessions.open(sessionId);
        const outs = session.files;

        const probes = [];
        probes.push(await probe("list_len", async () => (await outs.list()).files.length));
        probes.push(await probe("find_all_len", async () => (await outs.find({})).length));
        probes.push(await probe("last_undefined", async () => (await outs.last()) === undefined));
        probes.push(await probe("first_undefined", async () => (await outs.first()) === undefined));
        probes.push(await probe("findOne_null", async () => await outs.findOne({ filename: "whatever.txt" })));
        probes.push(await probe("read_missing", async () => await outs.read({ path: "whatever.txt", match: "suffix" })));
        probes.push(await probe("download_files_zip", async () => zipProbeNoTransientManifestErrors(await outs.download(undefined))));
        probes.push(await probe("download_all_zip", async () => zipProbeNoTransientManifestErrors(await session.download())));
        probes.push(await probe("download_metadata_zip", async () => zipProbe(await session.downloadMetadata())));

        process.stdout.write(JSON.stringify({ sessionId, status, probes }));
        process.exit(0);
      `;
      const r = await runChild(install, "edge-files-D.mjs", body, 9 * 60_000);
      const ctx = `\n\n${JSON.stringify(r, null, 2).slice(0, 3000)}`;
      const probes = r.probes as ProbeResult[];

      expect(r.status, `run did not succeed${ctx}`).toBe("succeeded");
      expect(byLabel(probes, "list_len").value, `list() not empty on a no-files session${ctx}`).toBe(0);
      expect(byLabel(probes, "find_all_len").value, `find({}) not empty on a no-files session${ctx}`).toBe(0);
      expect(byLabel(probes, "last_undefined").value, `last() should be undefined when empty${ctx}`).toBe(true);
      expect(byLabel(probes, "first_undefined").value, `first() should be undefined when empty${ctx}`).toBe(true);
      const fo = byLabel(probes, "findOne_null");
      expect(fo.ok && fo.value === null, `findOne on empty session must return null${ctx}`).toBe(true);
      const rm = byLabel(probes, "read_missing");
      expect(rm.ok, `read of a missing file must throw, not resolve${ctx}`).toBe(false);
      expect(rm.error!.message, `read of missing file hung${ctx}`).not.toContain("PROBE_TIMEOUT");
      for (const label of ["download_files_zip", "download_all_zip", "download_metadata_zip"]) {
        const p = byLabel(probes, label);
        expect(p.ok, `${label} threw on a no-files session: ${JSON.stringify(p.error)}${ctx}`).toBe(true);
        const z = p.value as { byteLength: number; magicOk: boolean; hasManifest: boolean; manifestErrors: unknown[] };
        expect(z.byteLength, `${label} empty on no-files session${ctx}`).toBeGreaterThan(0);
        expect(z.magicOk, `${label} not a valid zip on no-files session${ctx}`).toBe(true);
        expect(z.hasManifest, `${label} missing manifest.json on no-files session${ctx}`).toBe(true);
        expect(z.manifestErrors, `${label} manifest recorded per-artifact download errors on no-files session${ctx}`).toEqual([]);
      }
    },
    10 * 60_000
  );
});
