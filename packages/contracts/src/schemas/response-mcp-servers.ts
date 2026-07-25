/**
 * Response schemas for the `mcpServers.*` family — persisted workspace MCP
 * server configuration.
 *
 * `headerShape` is header NAMES only; the values live in the secret store. As
 * with secrets, the strict object is what keeps that true: a `headers` or
 * `authorization` key appearing on a read fails the suite.
 *
 * These four routes had NO declared client type: `operations.ts` has no
 * mcp-server operations, so the CLI/dashboard reached them directly and cast the
 * result. `McpServerRecord` / `McpServerList` in `runtime-types.ts` are now
 * `z.infer`red from the schemas below, so the contract has exactly one
 * statement. The shapes themselves come from the server projection in
 * `api/routes/mcp-servers.ts`.
 *
 * `workspaceId` is the PUBLIC `wsp_<hex>` form here (the handler runs it through
 * `publicWorkspaceId`), matching `whoami` — and NOT matching the billing ledger,
 * the admin-billing routes or the workspace erase, which all emit raw ids.
 */
import * as z from "zod/mini";
import {
  describeResponse,
  responseObject,
  wireNonEmptyString,
  wireString
} from "./response-common.js";

export const McpServerRecordSchema = describeResponse(
  "McpServerRecord",
  "One persisted workspace MCP server. `headerShape` lists header NAMES only.",
  responseObject({
    id: wireNonEmptyString,
    workspaceId: wireNonEmptyString,
    name: wireNonEmptyString,
    url: wireNonEmptyString,
    headerShape: z.array(wireString),
    createdAt: wireNonEmptyString,
    updatedAt: wireNonEmptyString
  })
);

export const McpServerResponseSchema = describeResponse(
  "McpServerResponse",
  "One workspace MCP server configuration.",
  responseObject({ mcpServer: McpServerRecordSchema })
);

export const McpServerListResponseSchema = describeResponse(
  "McpServerListResponse",
  "Every workspace MCP server, newest first. Not paged.",
  responseObject({ mcpServers: z.array(McpServerRecordSchema) })
);
