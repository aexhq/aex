import { createHash } from "node:crypto";
import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { builtinModules } from "node:module";
import { extname, isAbsolute, relative, resolve, sep } from "node:path";
import { pathToFileURL } from "node:url";

import { build, type BuildResult } from "esbuild";

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
  const entries = Object.entries(config.entries).sort(([left], [right]) => left.localeCompare(right, "en"));
  if (entries.length === 0) throw new TypeError("aex.tools.entries must contain at least one entry");
  const outputs: BuiltTool[] = [];
  for (const [exportName, sourceValue] of entries) {
    if (typeof sourceValue !== "string") throw new TypeError(`Tool entry ${JSON.stringify(exportName)} must be a path`);
    const outputName = outputNameOf(exportName);
    const source = inside(root, resolve(root, sourceValue), `Tool entry ${JSON.stringify(exportName)}`);
    if (!SOURCE_EXTENSIONS.has(extname(source))) throw new TypeError(`Unsupported Tool module extension ${extname(source) || "(none)"}`);
    outputs.push(await buildOne(root, outDir, outputName, source));
  }
  return Object.freeze(outputs);
}

async function buildOne(root: string, outDir: string, name: string, source: string): Promise<BuiltTool> {
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
  const hasSetup = typeof selected.setupHandler === "function";
  const runtime = await bundle({
    contents: `
import selected from ${JSON.stringify(sourceFromOutput)};
if (selected?.kind !== "aex.tool" || typeof selected.handler !== "function") throw new TypeError("invalid prepared Tool");
export default Object.freeze({
  kind: "tool-runtime/v1",
  name: selected.name,
  execute: selected.handler,
  setup: selected.setupHandler,
});
`,
    resolveDir: outDir,
    sourcefile: `${name}.runtime.mjs`,
  });
  const digest = createHash("sha256").update(runtime).digest("hex");
  const runtimeName = `${name}.${digest}.mjs`;
  await writeFile(resolve(outDir, runtimeName), runtime);
  const artifact = {
    digest,
    target: "linux-amd64",
    contentBase64: Buffer.from(runtime).toString("base64"),
    bytes: runtime.byteLength,
    execute: `/artifacts/${digest}/execute`,
    ...(hasSetup ? { setup: `/artifacts/${digest}/setup` } : {}),
  };
  const prepared = await bundle({
    contents: `
import selected from ${JSON.stringify(sourceFromOutput)};
import { withPreparedArtifact } from "@aexhq/sdk/internal";
export default withPreparedArtifact(selected, ${JSON.stringify(artifact)});
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
    target: "linux-amd64",
    digest,
    bytes: runtime.byteLength,
    execute: artifact.execute,
    ...(artifact.setup === undefined ? {} : { setup: artifact.setup }),
    blobs: [{ digest, file: runtimeName }],
  }, null, 2)}\n`);
  return Object.freeze({ name, output: relative(root, output).replaceAll(sep, "/"), digest, bytes: runtime.byteLength });
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
