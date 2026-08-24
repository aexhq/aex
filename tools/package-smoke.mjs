import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..");
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
    pack(path.join(root, "packages/session-protocol")),
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
import { callbacks, defineEnvironment, isEnvironmentRef } from "@aexhq/environment";
import { tool, type EnvironmentRef } from "@aexhq/sdk";
import { z } from "zod";

const application = defineEnvironment({
  identity: "package-smoke",
  protocol: "environment/v1",
  profile: callbacks(),
  serialize: () => ({}),
  handle: () => ({ close() {} }),
})();
const echo = tool(z.object({ value: z.string() }), async function echo(input) { return input; });
const bound = echo.bind(application);
const reference: EnvironmentRef = bound.environment;
assert.equal(reference, application);
assert.equal(isEnvironmentRef(reference), true);
console.log("packed Aex packages share one opaque EnvironmentRef identity");
`,
  );
  run(process.execPath, [path.join(consumer, "node_modules/typescript/bin/tsc")], { cwd: consumer });
  const output = run(process.execPath, ["dist/smoke.js"], { cwd: consumer });
  assert.match(output, /share one opaque EnvironmentRef identity/u);
  process.stdout.write(`${output}\n`);
} finally {
  await rm(temporary, { recursive: true, force: true });
}
