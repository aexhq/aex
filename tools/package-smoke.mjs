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
    pack(path.join(root, "packages/environment")),
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
      "--no-package-lock",
      "--no-audit",
      "--no-fund",
      ...packages,
      `@aexhq/brain@${brainVersion}`,
      "@types/node@24.3.0",
      "typescript@5.9.2",
      "zod@4.4.3",
    ],
    { cwd: consumer },
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
import { component } from "@aexhq/brain";
import { Aex } from "@aexhq/sdk";

const bytes = new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]);
const model = component("model", bytes, {}, { metadata: { name: "smoke" } });
const agentloop = component("agentloop", bytes, {});
const environment = component("environment", bytes, {});
const echo = component("tool", bytes, {
  definition: {
    name: "echo",
    input_schema: { type: "object" },
    output_schema: { type: "object" },
    contract_digest: "a".repeat(64),
  },
}, { grants: ["environment"] });

async function typecheckAex(aex: Aex): Promise<void> {
  await aex.sessions.create({
    model: { component: model, provider: "smoke", name: "smoke", apiKey: "not-used" },
    agentloop,
    environments: { workspace: environment },
    tools: [echo],
  });
}
void typecheckAex;
assert.equal(echo.extension, "tool");
console.log("packed Aex consumes Brain's four component values");
`,
  );
  run(process.execPath, [path.join(consumer, "node_modules/typescript/bin/tsc")], { cwd: consumer });
  const output = run(process.execPath, ["dist/smoke.js"], { cwd: consumer });
  assert.match(output, /four component values/u);
  process.stdout.write(`${output}\n`);
} finally {
  await rm(temporary, { recursive: true, force: true });
}
