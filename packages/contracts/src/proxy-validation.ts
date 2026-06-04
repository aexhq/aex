import type { PlatformProxyEndpoint, PlatformProxyEndpointAuth } from "./submission.js";

/**
 * Cross-validate a `proxyEndpoints` policy list against a
 * `secrets.proxyEndpointAuth` value list. Throws on the first mismatch
 * with an actionable, field-named error message.
 *
 * Mirrors the BFF's authoritative validator so misconfigured submissions
 * fail fast in the SDK/CLI before going over the wire.
 */
export function validateProxyAuth(
  endpoints: readonly PlatformProxyEndpoint[],
  auth: readonly PlatformProxyEndpointAuth[] | undefined
): void {
  const authList = auth ?? [];
  const endpointNames = new Set(endpoints.map((e) => e.name));
  const authNames = new Set<string>();
  for (const entry of authList) {
    if (authNames.has(entry.name)) {
      throw new Error(`secrets.proxyEndpointAuth contains duplicate name '${entry.name}'`);
    }
    authNames.add(entry.name);
    if (!endpointNames.has(entry.name)) {
      throw new Error(`secrets.proxyEndpointAuth[].name='${entry.name}' has no matching proxyEndpoints[].name`);
    }
  }
  for (const endpoint of endpoints) {
    const match = authList.find((a) => a.name === endpoint.name);
    if (!match) {
      throw new Error(`proxyEndpoints[].name='${endpoint.name}' is missing a matching secrets.proxyEndpointAuth entry`);
    }
    if (match.value.type !== endpoint.authShape.type) {
      throw new Error(
        `secrets.proxyEndpointAuth[name='${endpoint.name}'].value.type='${match.value.type}' does not match proxyEndpoints[name='${endpoint.name}'].authShape.type='${endpoint.authShape.type}'`
      );
    }
  }
}

/**
 * Build an `allowedHosts` list for `environment.network` that includes
 * the aex proxy host (and optionally Anthropic's MCP host) when the
 * caller is hand-rolling networking.
 */
export function buildPlatformAllowedHosts(input: {
  readonly baseUrl: string;
  readonly extraHosts?: readonly string[];
}): readonly string[] {
  const result: string[] = [];
  try {
    result.push(new URL(input.baseUrl).host);
  } catch {
    throw new Error("buildPlatformAllowedHosts: baseUrl must be an absolute URL");
  }
  for (const host of input.extraHosts ?? []) {
    if (!result.includes(host)) result.push(host);
  }
  return result;
}
