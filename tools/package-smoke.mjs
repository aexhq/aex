import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const brain = path.resolve(root, "../brain");
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
  // @aexhq/tools resolves its exact released brain-tools patch from the registry; Brain itself
  // remains the separately pinned 0.2 package source used by the SDK.
  const packages = [
    pack(path.join(brain, "packages/brain")),
    pack(path.join(root, "packages/contracts")),
    pack(path.join(root, "packages/sdk")),
    pack(path.join(root, "packages/tools")),
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
    path.join(consumer, "custom.ts"),
    `import { tool } from "@aexhq/sdk";
import { z } from "zod";

const custom = tool(
  z.object({ value: z.string() }),
  async function packageSmokeEcho(input) { return input; },
)
  .describe("Return the exact input.")
  .returns(z.object({ value: z.string() }))
  .server(import.meta.url);

export default custom;
`,
  );
  await writeFile(
    path.join(consumer, "smoke.ts"),
    `import assert from "node:assert/strict";
import { compileTools, tool as brainTool, type Tool } from "@aexhq/brain";
import { Aex, tool as aexTool } from "@aexhq/sdk";
import { bash } from "@aexhq/tools";
import custom from "./custom.js";

assert.equal(aexTool, brainTool, "Aex must re-export Brain's one Tool constructor");
const selected: readonly Tool[] = [custom, bash()];
const prepared = await compileTools(selected);
assert.deepEqual(prepared.items.map((item) => item.definition.name), ["packageSmokeEcho", "bash"]);

async function typecheckAex(aex: Aex): Promise<void> {
  await aex.sessions.create({
    model: { provider: "openai", name: "gpt-5", apiKey: "not-used" },
    tools: selected,
  });
}
void typecheckAex;
console.log("packed Aex and Brain packages share one executable Tool identity");
`,
  );
  run(process.execPath, [path.join(consumer, "node_modules/typescript/bin/tsc")], { cwd: consumer });
  const output = run(process.execPath, ["dist/smoke.js"], { cwd: consumer });
  assert.match(output, /share one executable Tool identity/u);
  process.stdout.write(`${output}\n`);
} finally {
  await rm(temporary, { recursive: true, force: true });
}
