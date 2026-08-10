import { mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";

const linkPath = ".vercel/project.json";
const outputConfigPath = ".vercel/output/config.json";
const localProject = `${JSON.stringify({
  orgId: "team_aex_public_build",
  projectId: "prj_aex_public_build",
  projectName: "dashboard",
  settings: {
    buildCommand: null,
    createdAt: 0,
    devCommand: null,
    directoryListing: false,
    framework: "nextjs",
    installCommand: null,
    nodeVersion: "22.x",
    outputDirectory: null,
    rootDirectory: "apps/dashboard",
  },
})}\n`;

function run(args) {
  const env = {
    ...process.env,
    NEXT_TELEMETRY_DISABLED: "1",
    VERCEL_TELEMETRY_DISABLED: "1",
  };
  for (const name of ["VERCEL_TOKEN", "VERCEL_ORG_ID", "VERCEL_PROJECT_ID"]) {
    delete env[name];
  }
  const completed = spawnSync("bun", args, {
    env,
    stdio: "inherit",
  });
  if (completed.error) throw completed.error;
  if (completed.status !== 0) process.exitCode = completed.status ?? 1;
}

try {
  await readFile(linkPath);
  throw new Error(`${linkPath} already exists; refusing to overwrite a provider project link`);
} catch (error) {
  if (error?.code !== "ENOENT") throw error;
}

await mkdir(".vercel", { recursive: true });
await writeFile(linkPath, localProject, { encoding: "utf8", flag: "wx" });
try {
  run(["run", "--filter", "@aexhq/sdk", "build"]);
  if (process.exitCode) throw new Error("dashboard SDK dependency build failed");
  run(["run", "vercel", "build", "--no-color"]);
  if (process.exitCode) throw new Error("Vercel Build Output API build failed");
  const config = JSON.parse(await readFile(outputConfigPath, "utf8"));
  if (config.version !== 3) {
    throw new Error(`${outputConfigPath} is not Build Output API version 3`);
  }
} finally {
  const current = await readFile(linkPath, "utf8").catch(() => null);
  if (current !== localProject) {
    throw new Error(`${linkPath} changed during the build; refusing to remove it`);
  }
  await rm(linkPath);
}
