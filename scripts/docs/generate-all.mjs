import { execFile } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { dirname, relative, resolve } from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

const execFileAsync = promisify(execFile);
const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, "..", "..");
const docsRoot = resolve(repoRoot, "apps", "docs");
const contentRoot = resolve(docsRoot, "content", "docs");
const publicRoot = resolve(docsRoot, "public");
const generatedRoot = resolve(docsRoot, ".generated");
const publicDocsBase = "https://aex.dev/docs";

const guideSources = [
  ["quickstart.md", "quickstart"],
  ["run-config.md", "run-config"],
  ["run-record.md", "run-record"],
  ["product-boundaries.md", "product-boundaries"],
  ["credentials.md", "credentials"],
  ["skills.md", "skills"],
  ["mcp.md", "mcp"],
  ["outputs.md", "outputs"],
  ["events.md", "events"],
  ["cleanup.md", "cleanup"],
  ["testing.md", "testing"],
  ["release.md", "release"]
];

const appDocRoutes = new Map([
  ...guideSources.map(([file, slug]) => [file, `/docs/guides/${slug}/`]),
  ["provider-runtime-capabilities.md", "/docs/reference/provider-runtime-capabilities/"]
]);

const referencePages = ["index", "sdk", "cli", "events", "provider-runtime-capabilities"];

await syncGuides();
await syncGeneratedCapabilityReference();
await generateCliReference();
await generateEventReference();
await generateSdkReference();
await generateLlmsFiles();

async function syncGuides() {
  const outDir = resolve(contentRoot, "guides");
  await resetDir(outDir);
  await writeFile(
    resolve(outDir, "meta.json"),
    `${JSON.stringify(
      {
        title: "Guides",
        icon: "PackageCheck",
        defaultOpen: true,
        pages: guideSources.map(([, slug]) => slug)
      },
      null,
      2
    )}\n`,
    "utf8"
  );
  for (const [file, slug] of guideSources) {
    const source = resolve(repoRoot, "packages", "sdk", "docs", file);
    const raw = await readFile(source, "utf8");
    const rewritten = rewritePackageDocLinks(raw);
    await writeFile(resolve(outDir, `${slug}.md`), rewritten, "utf8");
  }
}

async function syncGeneratedCapabilityReference() {
  const outDir = resolve(contentRoot, "reference");
  await mkdir(outDir, { recursive: true });
  await writeFile(
    resolve(outDir, "meta.json"),
    `${JSON.stringify(
      {
        title: "Reference",
        icon: "Braces",
        defaultOpen: true,
        pages: referencePages
      },
      null,
      2
    )}\n`,
    "utf8"
  );

  const source = resolve(repoRoot, "packages", "sdk", "docs", "provider-runtime-capabilities.md");
  const raw = await readFile(source, "utf8");
  await writeFile(resolve(outDir, "provider-runtime-capabilities.md"), rewritePackageDocLinks(raw), "utf8");
}

async function generateCliReference() {
  const cli = resolve(repoRoot, "packages", "sdk", "dist", "cli.mjs");
  if (!existsSync(cli) || (await latestSourceMtime()) > (await mtimeMs(cli))) {
    await runPnpm(["--filter", "@aexhq/contracts", "run", "build"]);
    await runPnpm(["--filter", "@aexhq/sdk", "run", "build"]);
  }
  const { stdout } = await execFileAsync(process.execPath, [cli, "--help"], {
    cwd: repoRoot,
    maxBuffer: 1024 * 1024
  });
  await writeMarkdown(resolve(contentRoot, "reference", "cli.md"), {
    title: "CLI",
    description: "Generated aex command-line reference.",
    body: [
      "# CLI",
      "",
      "Generated from `aex --help`.",
      "",
      "```text",
      stdout.trimEnd(),
      "```",
      ""
    ].join("\n")
  });
}

async function generateEventReference() {
  const envelope = await readFile(resolve(repoRoot, "packages", "contracts", "src", "event-envelope.ts"), "utf8");

  const eventTypes = readConstArray(envelope, "AEX_EVENT_TYPES");
  const sources = readConstArray(envelope, "AEX_EVENT_SOURCES");
  const channels = readConstArray(envelope, "AEX_EVENT_CHANNELS");
  const levels = readConstArray(envelope, "AEX_LOG_LEVELS");

  const body = [
    "# Events",
    "",
    "Generated from `packages/contracts/src/event-envelope.ts`.",
    "",
    "## Coordinator Envelope Types",
    "",
    markdownList(eventTypes),
    "",
    "## Sources",
    "",
    markdownList(sources),
    "",
    "## Channels",
    "",
    markdownList(channels),
    "",
    "## Log Levels",
    "",
    markdownList(levels),
    ""
  ].join("\n");

  await writeMarkdown(resolve(contentRoot, "reference", "events.md"), {
    title: "Events",
    description: "Generated event vocabulary reference.",
    body
  });
}

async function generateSdkReference() {
  await resetDir(resolve(contentRoot, "reference", "sdk"));
  await mkdir(generatedRoot, { recursive: true });
  const jsonPath = resolve(generatedRoot, "sdk-typedoc.json");
  await runPnpm([
    "--dir",
    resolve(repoRoot, "apps", "docs"),
    "exec",
    "typedoc",
    "--json",
    toPosixPath(jsonPath),
    "--entryPoints",
    toPosixPath(resolve(repoRoot, "packages", "sdk", "src", "index.ts")),
    "--tsconfig",
    toPosixPath(resolve(repoRoot, "packages", "sdk", "tsconfig.build.json")),
    "--excludePrivate",
    "--excludeProtected",
    "--excludeInternal",
    "--skipErrorChecking",
    "--sourceLinkTemplate",
    "https://github.com/aexhq/aex/blob/{gitRevision}/{path}#L{line}",
    "--hideGenerator"
  ]);
  const typedoc = JSON.parse(await readFile(jsonPath, "utf8"));
  const exported = flattenTypedoc(typedoc).filter((item) => isPublicDocItem(item));
  const groups = groupByKind(exported);

  const sections = ["Class", "Interface", "Function", "TypeAlias", "Variable", "Enumeration"]
    .filter((kind) => groups.has(kind))
    .map((kind) => renderKindSection(kind, groups.get(kind)));

  await writeMarkdown(resolve(contentRoot, "reference", "sdk", "index.md"), {
    title: "SDK",
    description: "Generated TypeScript SDK reference.",
    body: [
      "# SDK",
      "",
      "Generated from `packages/sdk/src/index.ts` with TypeDoc.",
      "",
      ...sections,
      ""
    ].join("\n")
  });
}

async function generateLlmsFiles() {
  await mkdir(publicRoot, { recursive: true });
  const summary = [
    "# aex",
    "",
    "> TypeScript SDK and CLI for durable autonomous agent runs across Anthropic, DeepSeek, OpenAI, Gemini, and Mistral.",
    "",
    "aex accepts one run submission shape, routes every provider through the managed runtime, emits one event stream, and returns captured outputs and logs.",
    "",
    "## Start",
    "",
    `- [Overview](${publicDocsBase}/)`,
    `- [Quickstart](${publicDocsBase}/guides/quickstart/)`,
    `- [Product capabilities and boundaries](${publicDocsBase}/guides/product-boundaries/)`,
    `- [Runs](${publicDocsBase}/concepts/runs/)`,
    `- [Providers & Runtimes](${publicDocsBase}/concepts/providers-and-runtimes/)`,
    `- [Secrets & BYOK](${publicDocsBase}/concepts/secrets-byok/)`,
    "",
    "## Guides",
    "",
    ...guideSources.map(([, slug]) => `- [${titleForSlug(slug)}](${publicDocsBase}/guides/${slug}/)`),
    "",
    "## Reference",
    "",
    `- [SDK](${publicDocsBase}/reference/sdk/)`,
    `- [CLI](${publicDocsBase}/reference/cli/)`,
    `- [Events](${publicDocsBase}/reference/events/)`,
    `- [Provider Runtime Capabilities](${publicDocsBase}/reference/provider-runtime-capabilities/)`,
    ""
  ].join("\n");
  await writeFile(resolve(publicRoot, "llms.txt"), summary, "utf8");

  const fullParts = [summary];
  for (const [file, slug] of guideSources) {
    fullParts.push(`\n\n# ${titleForSlug(slug)}\n`);
    fullParts.push(await readFile(resolve(repoRoot, "packages", "sdk", "docs", file), "utf8"));
  }
  fullParts.push("\n\n# Provider Runtime Capabilities\n");
  fullParts.push(await readFile(resolve(repoRoot, "packages", "sdk", "docs", "provider-runtime-capabilities.md"), "utf8"));
  await writeFile(resolve(publicRoot, "llms-full.txt"), fullParts.join("\n"), "utf8");
}

function rewritePackageDocLinks(input, sourceDir = resolve(repoRoot, "packages", "sdk", "docs")) {
  let out = input.replace(/\]\(([^)#]+\.md)(#[^)]+)?\)/g, (match, href, hash = "") => {
    const basename = href.replace(/\\/g, "/").split("/").at(-1);
    if (appDocRoutes.has(basename)) {
      return `](${appDocRoutes.get(basename)}${hash})`;
    }
    if (isRelativeHref(href)) return repoSourceLink(match, sourceDir, href, hash);
    return match;
  });
  out = out.replace(/\]\(((?:\.\.?[/\\])[^)#]+)(#[^)]+)?\)/g, (match, href, hash = "") => {
    if (!isRelativeHref(href)) return match;
    return repoSourceLink(match, sourceDir, href, hash);
  });
  return out;
}

function isRelativeHref(href) {
  return href.startsWith("../") || href.startsWith("..\\") || href.startsWith("./") || href.startsWith(".\\");
}

function repoSourceLink(match, sourceDir, href, hash = "") {
  const target = resolve(sourceDir, href);
  const relPath = relative(repoRoot, target);
  if (relPath.startsWith("..")) return match;
  return `](https://github.com/aexhq/aex/blob/main/${toPosixPath(relPath)}${hash})`;
}

function readConstArray(source, name) {
  const match = source.match(new RegExp(`export const ${name} = \\[([\\s\\S]*?)\\] as const`));
  if (!match) return [];
  const withoutComments = match[1]
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/\/\/.*$/gm, "");
  return [...withoutComments.matchAll(/"([^"]+)"/g)].map((m) => m[1]);
}

function flattenTypedoc(node, acc = []) {
  if (!node || typeof node !== "object") return acc;
  if (node.kind && node.name) acc.push(node);
  for (const child of node.children ?? []) flattenTypedoc(child, acc);
  return acc;
}

function isPublicDocItem(item) {
  if (!item.name || item.name.startsWith("_") || item.name === "default") return false;
  if (item.flags?.isPrivate || item.flags?.isProtected) return false;
  return ["Class", "Interface", "Function", "Type alias", "Variable", "Enumeration"].includes(kindName(item));
}

function groupByKind(items) {
  const groups = new Map();
  for (const item of items) {
    const key = kindName(item).replace(/\s+/g, "");
    const arr = groups.get(key) ?? [];
    arr.push(item);
    groups.set(key, arr);
  }
  for (const arr of groups.values()) arr.sort((a, b) => a.name.localeCompare(b.name));
  return groups;
}

function renderKindSection(kind, items = []) {
  const labels = new Map([
    ["Class", "Classes"],
    ["Interface", "Interfaces"],
    ["Function", "Functions"],
    ["TypeAlias", "Type Aliases"],
    ["Variable", "Variables"],
    ["Enumeration", "Enumerations"]
  ]);
  const label = labels.get(kind) ?? `${kind}s`;
  const lines = [`## ${label}`, ""];
  for (const item of items) {
    lines.push(`### \`${item.name}\``, "");
    const summary = commentText(item.comment) || signatureComment(item) || "";
    if (summary) lines.push(summary, "");
    const signatures = item.signatures ?? [];
    for (const signature of signatures) {
      const params = (signature.parameters ?? []).map((param) => param.name).join(", ");
      lines.push(`\`${item.name}(${params})\``, "");
    }
    const members = (item.children ?? []).filter((child) => isPublicMember(child));
    if (members.length > 0) {
      lines.push("Members:", "");
      for (const member of members) {
        lines.push(`- \`${member.name}\`${kindName(member) ? ` (${kindName(member)})` : ""}`);
      }
      lines.push("");
    }
  }
  return lines.join("\n");
}

function isPublicMember(member) {
  if (!member.name || member.name.startsWith("_") || member.name.startsWith("#")) return false;
  if (member.flags?.isPrivate || member.flags?.isProtected) return false;
  if (member.inheritedFrom) return false;
  return ["Method", "Property", "Accessor", "Constructor", "Function"].includes(kindName(member));
}

function kindName(item) {
  if (item.kindString) return item.kindString;
  const names = new Map([
    [4, "Namespace"],
    [8, "Enumeration"],
    [16, "Enumeration member"],
    [32, "Variable"],
    [64, "Function"],
    [128, "Class"],
    [256, "Interface"],
    [512, "Constructor"],
    [1024, "Property"],
    [2048, "Method"],
    [262144, "Accessor"],
    [4194304, "Type alias"]
  ]);
  return names.get(item.kind) ?? "";
}

function commentText(comment) {
  const parts = comment?.summary ?? [];
  return parts.map((part) => part.text ?? "").join("").trim();
}

function signatureComment(item) {
  return commentText(item.signatures?.[0]?.comment);
}

function markdownList(values) {
  return values.length > 0 ? values.map((value) => `- \`${value}\``).join("\n") : "_No generated values found._";
}

async function writeMarkdown(path, { title, description, body }) {
  await writeFile(
    path,
    `---\ntitle: ${JSON.stringify(title)}\ndescription: ${JSON.stringify(description)}\n---\n\n${body}`,
    "utf8"
  );
}

async function resetDir(path) {
  await rm(path, { recursive: true, force: true });
  await mkdir(path, { recursive: true });
}

async function runPnpm(args) {
  const command = process.platform === "win32" ? "cmd.exe" : "pnpm";
  const commandArgs = process.platform === "win32" ? ["/d", "/s", "/c", "pnpm", ...args] : args;
  await execFileAsync(command, commandArgs, {
    cwd: repoRoot,
    maxBuffer: 20 * 1024 * 1024
  });
}

function titleForSlug(slug) {
  if (slug === "mcp") return "MCP";
  return slug
    .split("-")
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ");
}

function toPosixPath(path) {
  return path.replace(/\\/g, "/");
}

async function latestSourceMtime() {
  const roots = [
    resolve(repoRoot, "packages", "cli", "src"),
    resolve(repoRoot, "packages", "sdk", "src"),
    resolve(repoRoot, "packages", "contracts", "src")
  ];
  let latest = 0;
  for (const root of roots) {
    latest = Math.max(latest, await latestMtimeUnder(root));
  }
  return latest;
}

async function latestMtimeUnder(path) {
  const info = await stat(path);
  if (info.isFile()) return info.mtimeMs;
  let latest = info.mtimeMs;
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const child = resolve(path, entry.name);
    if (entry.isDirectory()) latest = Math.max(latest, await latestMtimeUnder(child));
    if (entry.isFile()) latest = Math.max(latest, (await stat(child)).mtimeMs);
  }
  return latest;
}

async function mtimeMs(path) {
  return (await stat(path)).mtimeMs;
}
