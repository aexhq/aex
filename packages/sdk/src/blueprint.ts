import type {
  JsonValue,
  PlatformCleanupPolicy,
  PlatformEnvironment
} from "@antpath/contracts";
import type { McpServer } from "./mcp-server.js";
import type { ProxyEndpoint } from "./proxy-endpoint.js";
import type { Skill } from "./skill.js";

/**
 * @internal Legacy compatibility shape retained until a breaking SDK cleanup.
 * It is not exported from the public package root; callers should submit
 * ordinary `submitRun` options or their own app-level helper return values.
 */
export interface Blueprint {
  readonly model: string;
  readonly system?: string;
  readonly prompt: string | readonly string[];
  readonly skills?: readonly Skill[];
  readonly mcpServers?: readonly McpServer[];
  readonly environment?: PlatformEnvironment;
  readonly cleanup?: PlatformCleanupPolicy;
  readonly proxyEndpoints?: readonly ProxyEndpoint[];
  readonly metadata?: Record<string, JsonValue>;
}

/**
 * @internal Legacy identity wrapper retained only for compatibility with
 * older builds. It is intentionally absent from the public root export.
 */
export function defineRun<TParams>(producer: (params: TParams) => Blueprint): (params: TParams) => Blueprint {
  if (typeof producer !== "function") {
    throw new TypeError("defineRun expects a function");
  }
  return (params: TParams): Blueprint => producer(params);
}
