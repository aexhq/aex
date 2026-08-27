#!/usr/bin/env node

import { chmod, mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import process from "node:process";

import { Aex, AexError } from "@aexhq/sdk";

const HELP = `Aex — the session backend for AI apps

Usage:
  aex login
  aex doctor
  aex session list
  aex session get <session-id>
  aex session send <session-id> <message>
  aex session events <session-id>
  aex session cancel <session-id>
  aex session end <session-id>
  aex session delete <session-id>

The API defaults to https://api.aex.dev.`;

interface StoredConfig {
  apiKey: string;
}

async function main(argv: string[]): Promise<void> {
  const [command, ...rest] = argv;
  if (command === undefined || command === "help" || command === "--help" || command === "-h") {
    process.stdout.write(`${HELP}\n`);
    return;
  }
  if (command === "login") {
    await login();
    return;
  }
  const aex = await client();
  if (command === "doctor") {
    await aex.listSessions();
    process.stdout.write("Aex API: ok\n");
    return;
  }
  if (command !== "session") usage(`Unknown command: ${command}`);
  await sessionCommand(aex, rest);
}

async function sessionCommand(aex: Aex, argv: string[]): Promise<void> {
  const [command, id, ...tail] = argv;
  switch (command) {
    case "list": {
      print(await aex.listSessions());
      return;
    }
    case "get": {
      requireId(id, command);
      print((await aex.getSession(id)).state);
      return;
    }
    case "send": {
      requireId(id, command);
      const message = tail.join(" ").trim();
      if (message === "") usage("session send requires a message");
      const session = await aex.getSession(id);
      print(await session.send(message));
      return;
    }
    case "events": {
      requireId(id, command);
      const session = await aex.getSession(id);
      const controller = new AbortController();
      process.once("SIGINT", () => controller.abort());
      try {
        for await (const event of session.events()) {
          process.stdout.write(`${JSON.stringify(event)}\n`);
        }
      } catch (error) {
        if (!controller.signal.aborted) throw error;
      }
      return;
    }
    case "cancel": {
      requireId(id, command);
      const session = await aex.getSession(id);
      await session.cancel();
      print(session.state);
      return;
    }
    case "end": {
      requireId(id, command);
      const session = await aex.getSession(id);
      print(await session.end());
      return;
    }
    case "delete": {
      requireId(id, command);
      const session = await aex.getSession(id);
      await session.delete();
      return;
    }
    default:
      usage(command === undefined ? "Missing session command" : `Unknown session command: ${command}`);
  }
}

async function client(): Promise<Aex> {
  const apiKey = process.env.AEX_API_KEY ?? (await readConfig())?.apiKey;
  if (apiKey === undefined || apiKey === "") {
    throw new Error("No Aex API key. Run `aex login` first.");
  }
  return new Aex({
    apiKey,
    ...(process.env.AEX_BASE_URL === undefined ? {} : { baseUrl: process.env.AEX_BASE_URL }),
  });
}

async function login(): Promise<void> {
  const apiKey = (await readSecret("Paste Aex API key: ")).trim();
  if (!/^aex_sk_[A-Za-z0-9]{40,64}$/.test(apiKey)) {
    throw new Error("That does not look like an Aex session API key.");
  }
  const path = configPath();
  await mkdir(dirname(path), { recursive: true, mode: 0o700 });
  const temporary = `${path}.${process.pid}.tmp`;
  await writeFile(temporary, `${JSON.stringify({ apiKey } satisfies StoredConfig, null, 2)}\n`, {
    encoding: "utf8",
    mode: 0o600,
  });
  await rename(temporary, path);
  await chmod(path, 0o600).catch(() => undefined);
  process.stdout.write("Saved.\n");
}

async function readConfig(): Promise<StoredConfig | undefined> {
  try {
    const value = JSON.parse(await readFile(configPath(), "utf8")) as Partial<StoredConfig>;
    return typeof value.apiKey === "string" ? { apiKey: value.apiKey } : undefined;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return undefined;
    throw error;
  }
}

function configPath(): string {
  return process.env.AEX_CONFIG ?? join(homedir(), ".aex", "config.json");
}

async function readSecret(prompt: string): Promise<string> {
  if (!process.stdin.isTTY) {
    const chunks: Buffer[] = [];
    for await (const chunk of process.stdin) chunks.push(Buffer.from(chunk));
    return Buffer.concat(chunks).toString("utf8");
  }
  process.stdout.write(prompt);
  process.stdin.setRawMode(true);
  process.stdin.resume();
  return new Promise((resolveValue, reject) => {
    let value = "";
    const cleanup = (): void => {
      process.stdin.off("data", onData);
      process.stdin.setRawMode(false);
      process.stdin.pause();
      process.stdout.write("\n");
    };
    const onData = (chunk: Buffer): void => {
      for (const byte of chunk) {
        if (byte === 3) {
          cleanup();
          reject(new Error("Login cancelled."));
          return;
        }
        if (byte === 13 || byte === 10) {
          cleanup();
          resolveValue(value);
          return;
        }
        if (byte === 8 || byte === 127) {
          if (value.length > 0) {
            value = value.slice(0, -1);
            process.stdout.write("\b \b");
          }
          continue;
        }
        value += String.fromCharCode(byte);
        process.stdout.write("*");
      }
    };
    process.stdin.on("data", onData);
  });
}

function requireId(id: string | undefined, command: string): asserts id is string {
  if (id === undefined || id === "") usage(`session ${command} requires a session id`);
}

function print(value: unknown): void {
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
}

function usage(message: string): never {
  throw new Error(`${message}\n\n${HELP}`);
}

main(process.argv.slice(2)).catch((error: unknown) => {
  if (error instanceof AexError) {
    process.stderr.write(`${error.name}: ${error.message}${error.code === undefined ? "" : ` (${error.code})`}\n`);
  } else {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  }
  process.exitCode = 1;
});
