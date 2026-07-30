import { createHash } from "node:crypto";
import { constants, createReadStream } from "node:fs";
import {
  access,
  appendFile,
  readFile,
  rename,
  stat,
  writeFile
} from "node:fs/promises";
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
  appendFile: (path, data) => appendFile(path, data),
  fileSize: async (path) => (await stat(path)).size,
  sha256File: async (path) => {
    const digest = createHash("sha256");
    for await (const chunk of createReadStream(path)) digest.update(chunk);
    return `sha256:${digest.digest("hex")}`;
  },
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
