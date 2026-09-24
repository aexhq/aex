export const token = process.env.AEX_ENVIRONMENT_TOKEN;
if (!token || token.length < 32) throw new Error("AEX_ENVIRONMENT_TOKEN requires at least 32 characters");
const origin = new URL(process.env.AEX_OPERATOR_URL ?? "http://127.0.0.1:8082");
if (origin.protocol !== "http:" || !["127.0.0.1", "[::1]"].includes(origin.hostname)
  || origin.username || origin.password || origin.search || origin.hash || origin.pathname !== "/") {
  throw new Error("AEX_OPERATOR_URL must be a literal loopback HTTP origin");
}
export async function control(path, body, signal) {
  const timeout = AbortSignal.timeout(10_000);
  const response = await fetch(new URL(`/environments/${path}`, origin), {
    method: body === undefined ? "GET" : "POST", redirect: "error",
    signal: signal === undefined ? timeout : AbortSignal.any([timeout, signal]),
    headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  if (!response.ok) throw new Error(`Environment ${path} rejected: HTTP ${response.status}`);
  return response.json();
}
