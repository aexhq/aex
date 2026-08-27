import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const brainVersion = JSON.parse(await readFile(path.join(root, "package.json"), "utf8"))
  .devDependencies?.["@aexhq/brain"];
if (!/^\d+\.\d+\.\d+$/u.test(brainVersion)) {
  throw new Error("package smoke requires one exact published Brain version");
}
const npmCli = process.env.npm_execpath;
if (npmCli === undefined) throw new Error("run package-smoke through npm so its CLI is discoverable");
const brainPackage = process.env.BRAIN_PACKAGE_ARCHIVE ?? `@aexhq/brain@${brainVersion}`;
const temporary = await mkdtemp(path.join(tmpdir(), "aex-package-smoke-"));
const artifacts = path.join(temporary, "artifacts");
const consumer = path.join(temporary, "consumer");

const run = (command, args, options = {}) =>
  execFileSync(command, args, { encoding: "utf8", stdio: "pipe", ...options }).trim();
const runNpm = (args, options = {}) => run(process.execPath, [npmCli, ...args], options);

const pack = (directory) => {
  const filename = runNpm(["pack", "--silent", "--pack-destination", artifacts], {
    cwd: directory,
  }).split(/\r?\n/u).at(-1);
  if (filename === undefined || !filename.endsWith(".tgz")) {
    throw new Error(`npm pack returned no archive for ${directory}`);
  }
  return path.join(artifacts, filename);
};

try {
  await mkdir(artifacts);
  await mkdir(consumer);
  const packages = [
    pack(path.join(root, "packages/contracts")),
    pack(path.join(root, "packages/sdk")),
    pack(path.join(root, "packages/cli")),
  ];

  await writeFile(
    path.join(consumer, "package.json"),
    `${JSON.stringify({ name: "aex-clean-consumer", private: true, type: "module" }, null, 2)}\n`,
  );
  runNpm(
    [
      "install",
      "--no-audit",
      "--no-fund",
      ...packages,
      brainPackage,
      "@types/node@24.3.0",
      "typescript@5.9.2",
      "zod@4.4.3",
    ],
    { cwd: consumer },
  );
  runNpm(["audit", "--audit-level=high"], { cwd: consumer });

  // A workspace `overrides` entry collapses every Brain copy here but never in a customer's
  // install, so a stale transitive pin ships a second Brain whose component contract digests
  // reject the components this SDK builds. Judge the packed tree, which has no override.
  const tree = JSON.parse(
    runNpm(["ls", "@aexhq/brain", "--all", "--json"], { cwd: consumer }),
  );
  const resolved = new Set();
  const walk = (node) => {
    for (const [name, child] of Object.entries(node.dependencies ?? {})) {
      if (name === "@aexhq/brain" && child.version !== undefined) resolved.add(child.version);
      walk(child);
    }
  };
  walk(tree);
  assert.deepEqual(
    [...resolved],
    [brainVersion],
    `the packed dependency tree must resolve exactly one @aexhq/brain, got ${[...resolved]}`,
  );

  await writeFile(
    path.join(consumer, "tsconfig.json"),
    `${JSON.stringify({
      compilerOptions: {
        target: "ES2022",
        module: "NodeNext",
        moduleResolution: "NodeNext",
        strict: true,
        outDir: "dist",
      },
      include: ["*.ts"],
    }, null, 2)}\n`,
  );
  await writeFile(
    path.join(consumer, "smoke.ts"),
    `import assert from "node:assert/strict";
import { brain, installExtensionIdentity } from "@aexhq/brain";
import { Aex, type CreateSessionOptions } from "@aexhq/sdk";

const diagnostic = brain((author) => {
  author.on.message((_message, turn) => turn.done());
});
installExtensionIdentity(diagnostic, "diagnostic", new Uint8Array([1]));
const options: CreateSessionOptions = {
  model: { provider: "vercel-ai-gateway", name: "openai/gpt-5-mini", apiKey: "test-key" },
  brain: diagnostic(),
};

async function typecheckAex(aex: Aex): Promise<void> {
  await aex.sessions.create(options);
}
void typecheckAex;
assert.equal(options.model.provider, "vercel-ai-gateway");
console.log("packed Aex consumes Brain's neutral session contract");
`,
  );
  run(process.execPath, [path.join(consumer, "node_modules/typescript/bin/tsc")], { cwd: consumer });
  const output = run(process.execPath, ["dist/smoke.js"], { cwd: consumer });
  assert.match(output, /neutral session contract/u);
  process.stdout.write(`${output}\n`);
} finally {
  await rm(temporary, { recursive: true, force: true });
}
