/**
 * Response schemas for the `mcpServers.*` family — persisted workspace MCP
 * server configuration.
 *
 * `headerShape` is header NAMES only; the values live in the secret store. As
 * with secrets, the strict object is what keeps that true: a `headers` or
 * `authorization` key appearing on a read fails the suite.
 *
 * There is no declared client type for these responses — `operations.ts` has no
 * mcp-server operations, so the CLI/dashboard reach these routes directly. The
 * shape below is derived from the server projection (`mcpRowToPublic`), and that
 * gap is itself worth reporting: four public routes with no client contract.
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
