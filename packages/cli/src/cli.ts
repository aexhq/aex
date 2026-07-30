import { access, readFile, rename, writeFile } from "node:fs/promises";
import { constants } from "node:fs";
import { executeCli } from "./main.js";
import type { CliIO } from "./internal.js";

const io: CliIO = {
  argv: process.argv,
  cwd: () => process.cwd(),
  readFile: (path) => readFile(path, "utf8"),
  readFileBytes: async (path) => new Uint8Array(await readFile(path)),
  readStdin: async () => {
    const chunks: Buffer[] = [];
    for await (const chunk of process.stdin) chunks.push(Buffer.from(chunk));
    return Buffer.concat(chunks).toString("utf8");
  },
  stdinIsTTY: process.stdin.isTTY,
  writeFile: (path, data) => writeFile(path, data),
  renameFile: (from, to) => rename(from, to),
  fileExists: async (path) => {
    try {
      await access(path, constants.F_OK);
      return true;
    } catch {
      return false;
    }
  },
  fetchImpl: fetch,
  stdout: (chunk) => { process.stdout.write(chunk); },
  stdoutBytes: (chunk) => { process.stdout.write(chunk); },
  stderr: (chunk) => { process.stderr.write(chunk); },
  exit: (code) => { process.exitCode = code; }
};

await executeCli(io);
