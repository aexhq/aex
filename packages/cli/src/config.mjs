import { mkdir, chmod, writeFile, rename, readFile, rm } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { randomUUID } from "node:crypto";

const directory = () => process.env.AEX_CONFIG_DIR ?? join(process.platform === "win32" ? process.env.LOCALAPPDATA ?? homedir() : homedir(), ".aex");
export function origin(value) {
  const url = new URL(value);
  if ((url.protocol !== "https:" && !(url.protocol === "http:" && url.hostname === "127.0.0.1")) || url.username || url.password || url.pathname !== "/" || url.search || url.hash) throw new Error("Use an HTTPS origin (or http://127.0.0.1:PORT for local testing).");
  return url.origin;
}
export async function saveSession(session) {
  const dir = directory();
  await mkdir(dir, { recursive: true, mode: 0o700 });
  if (process.platform === "win32") {
    const exec = promisify(execFile);
    const { stdout } = await exec("whoami.exe", ["/user", "/fo", "csv", "/nh"], { windowsHide: true });
    const sid = stdout.match(/S-1-5-\d+(?:-\d+)+/)?.[0];
    if (!sid) throw new Error("Cannot determine Windows account for credential permissions.");
    await exec("icacls.exe", [dir, "/inheritance:r", "/grant:r", `*${sid}:(OI)(CI)F`, "*S-1-5-18:(OI)(CI)F"], { windowsHide: true });
  } else await chmod(dir, 0o700);
  const temporary = join(dir, `${randomUUID()}.tmp`);
  try {
    await writeFile(temporary, JSON.stringify(session) + "\n", { mode: 0o600, flag: "wx" });
    await rename(temporary, join(dir, "account.json"));
  } finally { await rm(temporary, { force: true }); }
}
export async function readSession() {
  try { return JSON.parse(await readFile(join(directory(), "account.json"), "utf8")); }
  catch (error) { if (error.code === "ENOENT") throw new Error("Sign in first with aex login."); throw error; }
}
export async function removeSession() { await rm(join(directory(), "account.json"), { force: true }); }
