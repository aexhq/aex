import { createHash } from "node:crypto";
import { mkdir, readFile, rename, rm, watch, writeFile } from "node:fs/promises";
import { builtinModules } from "node:module";
import { extname, isAbsolute, relative, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

import { build, type BuildResult } from "esbuild";
import * as z from "zod";

interface ToolBuildConfig {
  entries: Record<string, string>;
  outDir: string;
}

interface BuiltTool {
  name: string;
  output: string;
  digest: string;
  bytes: number;
}

const SOURCE_EXTENSIONS = new Set([".js", ".mjs", ".cjs", ".ts", ".mts", ".cts", ".tsx", ".jsx"]);
const builtins = new Set(builtinModules.flatMap((name) => [name, `node:${name}`]));
const NODE_VERSION = "22.23.2";
const NODE_RUNTIME_FILE = `node-v${NODE_VERSION}-linux-arm64.tar.xz`;
const NODE_RUNTIME_DIGEST = "fff4078c5def658577f92c88db7db3bc0072924bfb93fe52c1e744a54e94abb8";
const NODE_RUNTIME_URL = `https://nodejs.org/dist/v${NODE_VERSION}/${NODE_RUNTIME_FILE}`;

export async function buildTools(packageDirectory = process.cwd()): Promise<readonly BuiltTool[]> {
  const root = resolve(packageDirectory);
  const packageJsonPath = resolve(root, "package.json");
  const packageJson = JSON.parse(await readFile(packageJsonPath, "utf8")) as {
    aex?: { tools?: Partial<ToolBuildConfig> };
  };
  const config = packageJson.aex?.tools;
  if (config === undefined || !isRecord(config.entries) || typeof config.outDir !== "string") {
    throw new TypeError("package.json must declare aex.tools.entries and aex.tools.outDir");
  }
  const outDir = inside(root, resolve(root, config.outDir), "aex.tools.outDir");
  await mkdir(outDir, { recursive: true });
  const runtime = await ensureNodeRuntime(outDir);
  const entries = Object.entries(config.entries).sort(([left], [right]) => left.localeCompare(right, "en"));
  if (entries.length === 0) throw new TypeError("aex.tools.entries must contain at least one entry");
  const outputs: BuiltTool[] = [];
  for (const [exportName, sourceValue] of entries) {
    if (typeof sourceValue !== "string") throw new TypeError(`Tool entry ${JSON.stringify(exportName)} must be a path`);
    const outputName = outputNameOf(exportName);
    const source = inside(root, resolve(root, sourceValue), `Tool entry ${JSON.stringify(exportName)}`);
    if (!SOURCE_EXTENSIONS.has(extname(source))) throw new TypeError(`Unsupported Tool module extension ${extname(source) || "(none)"}`);
    outputs.push(await buildOne(root, outDir, outputName, source, runtime));
  }
  return Object.freeze(outputs);
}

export async function watchTools(
  packageDirectory = process.cwd(),
  onBuild: (built: readonly BuiltTool[]) => void = () => undefined,
): Promise<never> {
  const root = resolve(packageDirectory);
  const packageJson = JSON.parse(await readFile(resolve(root, "package.json"), "utf8")) as {
    aex?: { tools?: { outDir?: unknown } };
  };
  const configuredOutDir = packageJson.aex?.tools?.outDir;
  if (typeof configuredOutDir !== "string") throw new TypeError("package.json must declare aex.tools.outDir");
  const outputPrefix = `${relative(root, inside(root, resolve(root, configuredOutDir), "aex.tools.outDir"))
    .replaceAll(sep, "/")}/`;
  onBuild(await buildTools(root));
  let rebuilding = false;
  let pending = false;
  for await (const event of watch(root, { recursive: true })) {
    const filename = event.filename?.replaceAll("\\", "/");
    if (filename === undefined || filename.startsWith("node_modules/") || filename.startsWith(outputPrefix)) continue;
    if (rebuilding) {
      pending = true;
      continue;
    }
    do {
      pending = false;
      rebuilding = true;
      try {
        onBuild(await buildTools(root));
      } finally {
        rebuilding = false;
      }
    } while (pending);
  }
  throw new Error("Tool watch ended unexpectedly");
}

async function buildOne(
  root: string,
  outDir: string,
  name: string,
  source: string,
  nodeRuntime: { readonly file: string; readonly digest: string; readonly bytes: number },
): Promise<BuiltTool> {
  const sourceFromOutput = moduleSpecifier(relative(outDir, source));
  const inspection = await bundle({
    contents: `import selected from ${JSON.stringify(sourceFromOutput)}; export default selected;`,
    resolveDir: outDir,
    sourcefile: `${name}.inspect.mjs`,
  });
  const inspectionPath = resolve(outDir, `.${name}.${process.pid}.inspect.mjs`);
  await writeFile(inspectionPath, inspection);
  let selected: unknown;
  try {
    selected = (await import(`${pathToFileURL(inspectionPath).href}?build=${Date.now()}`)).default;
  } finally {
    await rm(inspectionPath, { force: true });
  }
  if (!isRecord(selected) || selected.kind !== "aex.tool" || typeof selected.handler !== "function") {
    throw new TypeError(`Tool entry ${JSON.stringify(name)} must default-export one completed Tool`);
  }
  const toolName = selected.name;
  if (typeof toolName !== "string") throw new TypeError(`Tool entry ${JSON.stringify(name)} must have a name`);
  const description = typeof selected.description === "string" ? selected.description : null;
  const inputSchema = z.toJSONSchema(selected.input as z.ZodType, { target: "draft-2020-12", unrepresentable: "throw" });
  const outputSchema = selected.output === undefined
    ? undefined
    : z.toJSONSchema(selected.output as z.ZodType, { target: "draft-2020-12", unrepresentable: "throw" });
  const contractDigest = createHash("sha256").update(canonicalJson({
    name: toolName,
    ...(description === null ? {} : { description }),
    input_schema: inputSchema,
    ...(outputSchema === undefined ? {} : { output_schema: outputSchema }),
  })).digest("hex");
  const requiredEnv = isRecord(selected.requirements) && Array.isArray(selected.requirements.env)
    ? selected.requirements.env
    : [];
  const hasSetup = typeof selected.setupHandler === "function";
  const runtime = await bundle({
    contents: `
import selected from ${JSON.stringify(sourceFromOutput)};
if (selected?.kind !== "aex.tool" || typeof selected.handler !== "function") throw new TypeError("invalid prepared Tool");
export default Object.freeze({
  kind: "tool-runtime/v1",
  name: ${JSON.stringify(toolName)},
  description: ${JSON.stringify(description)},
  contractDigest: ${JSON.stringify(contractDigest)},
  requiredEnv: ${JSON.stringify(requiredEnv)},
  execute: selected.handler,
  setup: selected.setupHandler,
});
`,
    resolveDir: outDir,
    sourcefile: `${name}.runtime.mjs`,
  });
  const codeDigest = createHash("sha256").update(runtime).digest("hex");
  const runtimeName = `${name}.${codeDigest}.mjs`;
  await writeFile(resolve(outDir, runtimeName), runtime);
  const layers = [
    {
      digest: nodeRuntime.digest,
      bytes: nodeRuntime.bytes,
      mediaType: "application/x-xz",
      mountPath: "/runtime",
      unpack: "tar.xz",
      file: nodeRuntime.file,
    },
    {
      digest: codeDigest,
      bytes: runtime.byteLength,
      mediaType: "application/javascript+esm",
      mountPath: "/tool/runtime.mjs",
      unpack: "file",
      file: runtimeName,
    },
  ] as const;
  const execute = "/tool/runtime.mjs";
  const setup = hasSetup ? "/tool/runtime.mjs" : undefined;
  const digest = createHash("sha256").update(canonicalJson({
    profile: "computer/v1",
    target: "linux-arm64",
    execute_path: execute,
    setup_path: setup ?? null,
    layers: layers.map(({ digest, bytes, mediaType, mountPath, unpack }) => ({
      checksum: digest,
      bytes,
      media_type: mediaType,
      mount_path: mountPath,
      unpack,
    })),
  })).digest("hex");
  const artifact = {
    digest,
    target: "linux-arm64",
    bytes: layers.reduce((total, layer) => total + layer.bytes, 0),
    execute,
    ...(setup === undefined ? {} : { setup }),
  };
  const artifactExpression = `{...${JSON.stringify(artifact)},layers:[${layers.map((layer) =>
    `{...${JSON.stringify({
      digest: layer.digest,
      bytes: layer.bytes,
      mediaType: layer.mediaType,
      mountPath: layer.mountPath,
      unpack: layer.unpack,
    })},source:new URL(${JSON.stringify(`./${layer.file}`)},import.meta.url)}`
  ).join(",")}]} `;
  const prepared = await bundle({
    contents: `
import selected from ${JSON.stringify(sourceFromOutput)};
import { withPreparedArtifact } from "@aexhq/sdk/internal";
export default withPreparedArtifact(selected, ${artifactExpression});
`,
    resolveDir: outDir,
    sourcefile: `${name}.prepared.mjs`,
    external: ["@aexhq/sdk", "@aexhq/sdk/internal", "zod"],
  });
  const output = resolve(outDir, `${name}.js`);
  await writeFile(output, prepared);
  await writeFile(resolve(outDir, `${name}.d.ts`), `import type { Tool } from "@aexhq/sdk";\ndeclare const value: Tool;\nexport default value;\n`);
  await writeFile(resolve(outDir, `${name}.artifact.json`), `${JSON.stringify({
    profile: "computer/v1",
    target: "linux-arm64",
    digest,
    bytes: artifact.bytes,
    execute: artifact.execute,
    ...(artifact.setup === undefined ? {} : { setup: artifact.setup }),
    blobs: layers.map(({ digest, file }) => ({ digest, file })),
  }, null, 2)}\n`);
  return Object.freeze({ name, output: relative(root, output).replaceAll(sep, "/"), digest, bytes: artifact.bytes });
}

async function ensureNodeRuntime(outDir: string): Promise<{ readonly file: string; readonly digest: string; readonly bytes: number }> {
  const path = resolve(outDir, NODE_RUNTIME_FILE);
  try {
    const bytes = await readFile(path);
    assertDigest(bytes, NODE_RUNTIME_DIGEST, "cached Node runtime");
    return Object.freeze({ file: NODE_RUNTIME_FILE, digest: NODE_RUNTIME_DIGEST, bytes: bytes.byteLength });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  const response = await fetch(NODE_RUNTIME_URL);
  if (!response.ok) throw new Error(`Could not download pinned Node runtime: HTTP ${response.status}`);
  const bytes = new Uint8Array(await response.arrayBuffer());
  assertDigest(bytes, NODE_RUNTIME_DIGEST, "downloaded Node runtime");
  const temporary = `${path}.${process.pid}.download`;
  await writeFile(temporary, bytes, { flag: "wx" });
  try {
    await rename(temporary, path);
  } catch (error) {
    await rm(temporary, { force: true });
    if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
  }
  return Object.freeze({ file: NODE_RUNTIME_FILE, digest: NODE_RUNTIME_DIGEST, bytes: bytes.byteLength });
}

function assertDigest(bytes: Uint8Array, expected: string, label: string): void {
  const actual = createHash("sha256").update(bytes).digest("hex");
  if (actual !== expected) throw new Error(`${label} SHA-256 mismatch`);
}

function canonicalJson(value: unknown): string {
  const visit = (current: unknown): unknown => {
    if (Array.isArray(current)) return current.map(visit);
    if (isRecord(current)) {
      return Object.fromEntries(Object.keys(current).sort().map((key) => [key, visit(current[key])]));
    }
    return current;
  };
  return JSON.stringify(visit(value));
}

async function bundle(options: {
  contents: string;
  resolveDir: string;
  sourcefile: string;
  external?: string[];
}): Promise<Uint8Array> {
  let result: BuildResult;
  try {
    result = await build({
      bundle: true,
      format: "esm",
      platform: "node",
      target: "node22",
      write: false,
      minify: true,
      keepNames: true,
      legalComments: "none",
      sourcemap: false,
      metafile: true,
      treeShaking: true,
      charset: "utf8",
      logLevel: "silent",
      ...(options.external === undefined ? {} : { external: options.external }),
      stdin: {
        contents: options.contents,
        resolveDir: options.resolveDir,
        sourcefile: options.sourcefile,
        loader: "js",
      },
    });
  } catch (cause) {
    throw new TypeError(`Tool ${options.sourcefile} could not be bundled for Node 22`, { cause });
  }
  if (result.warnings.length !== 0) throw new TypeError(result.warnings[0]?.text ?? "Tool build warning");
  for (const output of Object.values(result.metafile?.outputs ?? {})) {
    for (const imported of output.imports) {
      if (imported.external && !(options.external ?? []).includes(imported.path) && !builtins.has(imported.path)) {
        throw new TypeError(`Tool bundle leaves an unsupported runtime import: ${imported.path}`);
      }
    }
  }
  const output = result.outputFiles?.[0];
  if (output === undefined || result.outputFiles?.length !== 1) throw new TypeError("Tool build did not produce one ESM file");
  return output.contents;
}

function outputNameOf(value: string): string {
  if (!/^\.\/[A-Za-z0-9][A-Za-z0-9._/-]*$/u.test(value) || value.includes("..")) {
    throw new TypeError(`Invalid Tool export name ${JSON.stringify(value)}`);
  }
  const name = value.slice(2);
  if (name.includes("/")) throw new TypeError("MVP Tool export names cannot contain nested paths");
  return name;
}

function inside(root: string, candidate: string, label: string): string {
  const rel = relative(root, candidate);
  if (rel === "" || (!rel.startsWith(`..${sep}`) && rel !== ".." && !isAbsolute(rel))) return candidate;
  throw new TypeError(`${label} must stay inside the package directory`);
}

function moduleSpecifier(value: string): string {
  const normalized = value.replaceAll(sep, "/");
  return normalized.startsWith(".") ? normalized : `./${normalized}`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
