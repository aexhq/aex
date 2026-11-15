import { createServer } from "node:http";
import { createHash, randomBytes } from "node:crypto";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { Aex } from "@aexhq/sdk";

const exec = promisify(execFile);
export async function openBrowser(url) {
  const [command, args] = process.platform === "win32"
    ? ["rundll32.exe", ["url.dll,FileProtocolHandler", url]]
    : process.platform === "darwin" ? ["open", [url]] : ["xdg-open", [url]];
  await exec(command, args, { windowsHide: true });
}

export async function login({ apiUrl, siteUrl, onReady, signal = AbortSignal.timeout(600_000) }) {
  const verifier = randomBytes(32).toString("base64url"), state = randomBytes(32).toString("base64url");
  const server = createServer();
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const redirect = `http://127.0.0.1:${server.address().port}/callback`;
  const url = new URL("/cli", siteUrl);
  url.search = new URLSearchParams({ redirect_uri: redirect, state, code_challenge: createHash("sha256").update(verifier).digest("base64url") }).toString();
  try {
    return await new Promise((resolve, reject) => {
      let exchanging = false;
      const abort = () => reject(new Error("Login cancelled or timed out. Run aex login again."));
      signal.addEventListener("abort", abort, { once: true });
      server.once("close", () => signal.removeEventListener("abort", abort));
      server.on("request", async (request, response) => {
        response.setHeader("content-type", "text/plain; charset=utf-8");
        response.setHeader("cache-control", "no-store");
        response.setHeader("referrer-policy", "no-referrer");
        const callback = new URL(request.url, redirect);
        if (request.method !== "GET" || request.headers.host !== new URL(redirect).host || callback.pathname !== "/callback" || callback.searchParams.get("state") !== state || !callback.searchParams.get("code") || exchanging) {
          response.writeHead(400).end("Invalid login callback."); return;
        }
        exchanging = true;
        try {
          const session = await Aex.exchangeLogin({ code: callback.searchParams.get("code"), code_verifier: verifier, redirect_uri: redirect }, { baseUrl: apiUrl, timeoutMs: 15_000 });
          response.end("Signed in to Aex. You can close this tab and return to your terminal.");
          resolve(session);
        } catch (error) { response.writeHead(400).end("Login failed. Return to your terminal and try again."); reject(error); }
      });
      if (signal.aborted) abort();
      else Promise.resolve().then(() => onReady(url.toString())).catch(reject);
    });
  } finally { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); }
}
