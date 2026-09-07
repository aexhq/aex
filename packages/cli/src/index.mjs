#!/usr/bin/env node
import { parseArgs } from "node:util";
import { Aex } from "@aexhq/sdk";
import { login, openBrowser } from "./login.mjs";
import { origin, readSession, saveSession, removeSession } from "./config.mjs";

const help = `Usage: aex <command>

  login                    Open browser sign-in and save an account session
  logout                   Revoke this session and remove local credentials
  keys list                List API keys (secrets are never returned)
  keys create <name>        Create a key; save the returned token now
  keys rename <id> <name>   Rename a key
  keys revoke <id>          Revoke a key
  account                  Show account, limits and billing status
  billing                  Show billing status
  usage                    Show current usage
  docs                     Open the documentation

Results are JSON on stdout; login progress and errors go to stderr.
Options: --no-browser, --api-url <origin>, --site-url <origin>, --help
Automation: AEX_ACCOUNT_TOKEN and optional AEX_API_URL.
`;

async function main() {
  const { values, positionals } = parseArgs({ allowPositionals: true, options: { help: { type: "boolean", short: "h" }, "no-browser": { type: "boolean" }, "api-url": { type: "string" }, "site-url": { type: "string" } } });
  if (values.help || !positionals.length) { process.stdout.write(help); return; }
  const [command, operation, ...args] = positionals;
  const expected = { "keys list": 0, "keys create": 1, "keys rename": 2, "keys revoke": 1 };
  if (command === "keys" ? expected[`keys ${operation}`] !== args.length : !["login", "logout", "account", "billing", "usage", "docs"].includes(command) || positionals.length !== 1) throw new Error("Unknown command or incorrect arguments. Run aex --help.");
  const explicitApi = values["api-url"] ?? process.env.AEX_API_URL;
  const siteUrl = origin(values["site-url"] ?? "https://aex.dev");
  if (command === "docs") {
    const url = `${siteUrl}/docs`; process.stdout.write(url + "\n");
    if (!values["no-browser"]) await openBrowser(url);
    return;
  }
  if (command === "login") {
    const apiUrl = origin(explicitApi ?? "https://api.aex.dev");
    const controller = new AbortController(), timeout = setTimeout(() => controller.abort(), 600_000);
    const cancel = () => controller.abort(); process.once("SIGINT", cancel);
    try {
      const session = await login({ apiUrl, siteUrl, signal: controller.signal, onReady: async url => {
        process.stderr.write(`Sign in using this browser on this computer:\n${url}\n`);
        if (!values["no-browser"]) await openBrowser(url).catch(() => process.stderr.write("Could not open a browser. Open the URL above to continue.\n"));
      } });
      const client = new Aex({ accountToken: session.token, baseUrl: apiUrl, timeoutMs: 15_000 });
      try { await saveSession({ ...session, apiUrl }); }
      catch (error) { await client.account.logout(); throw error; }
      process.stdout.write(JSON.stringify(await client.account.get(), null, 2) + "\n");
    } finally { clearTimeout(timeout); process.removeListener("SIGINT", cancel); }
    return;
  }
  const session = process.env.AEX_ACCOUNT_TOKEN ? { token: process.env.AEX_ACCOUNT_TOKEN, apiUrl: origin(explicitApi ?? "https://api.aex.dev") } : await readSession();
  const apiUrl = origin(explicitApi ?? session.apiUrl);
  if (apiUrl !== session.apiUrl) throw new Error("Saved credentials belong to a different API origin. Run aex login for this origin.");
  const client = new Aex({ accountToken: session.token, baseUrl: apiUrl, timeoutMs: 15_000 });
  let result;
  if (command === "logout") {
    try { await client.account.logout(); } catch (error) { if (error.status !== 401) throw error; }
    if (!process.env.AEX_ACCOUNT_TOKEN) await removeSession();
    result = { signed_out: true };
  } else if (command === "account") result = await client.account.get();
  else if (command === "billing") result = { billing: (await client.account.get()).billing };
  else if (command === "usage") result = await client.account.usage();
  else if (operation === "list") result = await client.keys.list();
  else if (operation === "create") result = await client.keys.create({ name: args[0] });
  else if (operation === "rename") result = await client.keys.update(args[0], { name: args[1] });
  else { await client.keys.delete(args[0]); result = { revoked: args[0] }; }
  process.stdout.write(JSON.stringify(result, null, 2) + "\n");
}
main().catch(error => { process.stderr.write(`aex: ${error.message}\n`); process.exitCode = 1; });
