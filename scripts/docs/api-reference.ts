/**
 * P7 — render the HTTP API reference page from the GENERATED OpenAPI document.
 *
 * `docs/reference/` published SDK, CLI, event and capability pages and nothing
 * at all about the HTTP surface, so the 68-route data plane was undocumented to
 * a customer who is not using the TypeScript SDK. This closes that gap the only
 * way the repository allows: per `docs-gardening-2026-06-24` the page is
 * generated, and the fix for anything wrong on it is a fix to the schemas, the
 * route table, or this renderer — never to the page.
 *
 * The input is `packages/contracts/openapi/regional.json`, which
 * `openapi:generate` derives from the request schemas and the route table. This
 * module reads the document and nothing else, so it cannot drift from the spec,
 * and the spec cannot drift from the code the server runs.
 *
 * Written as a module rather than inline in `generate-all.mjs` so the rendering
 * is callable — and therefore testable — without running TypeDoc and the SDK
 * build that the rest of that pipeline needs.
 */

export const API_REFERENCE_SPEC_PATH = "packages/contracts/openapi/regional.json";
export const API_REFERENCE_PAGE_PATH = "apps/docs/content/docs/reference/api.md";
export const API_REFERENCE_TITLE = "HTTP API";
export const API_REFERENCE_DESCRIPTION = "Generated HTTP API reference for the aex data plane.";

/** The JSON Schema subset the generated document actually emits. */
export interface JsonSchemaNode {
  readonly $ref?: string;
  readonly type?: string | readonly string[];
  readonly enum?: readonly unknown[];
  readonly const?: unknown;
  readonly anyOf?: readonly JsonSchemaNode[];
  readonly oneOf?: readonly JsonSchemaNode[];
  readonly allOf?: readonly JsonSchemaNode[];
  readonly items?: JsonSchemaNode;
  readonly properties?: Readonly<Record<string, JsonSchemaNode>>;
  readonly required?: readonly string[];
  readonly additionalProperties?: boolean | JsonSchemaNode;
  readonly propertyNames?: JsonSchemaNode;
  readonly description?: string;
  readonly format?: string;
  readonly pattern?: string;
  readonly minLength?: number;
  readonly maxLength?: number;
  readonly minimum?: number;
  readonly maximum?: number;
  readonly minItems?: number;
  readonly maxItems?: number;
}

export interface OpenApiOperation {
  readonly operationId?: string;
  readonly summary?: string;
  readonly tags?: readonly string[];
  readonly parameters?: readonly {
    readonly name: string;
    readonly in: string;
    readonly required?: boolean;
  }[];
  readonly requestBody?: {
    readonly required?: boolean;
    readonly content?: Readonly<Record<string, { readonly schema?: JsonSchemaNode }>>;
  };
  readonly security?: readonly Readonly<Record<string, readonly string[]>>[];
  readonly responses?: Readonly<
    Record<string, { readonly content?: Readonly<Record<string, { readonly schema?: JsonSchemaNode }>> }>
  >;
}

export interface OpenApiDocument {
  readonly openapi: string;
  readonly info: { readonly title: string; readonly version: string; readonly description?: string };
  readonly servers?: readonly { readonly url: string; readonly description?: string }[];
  readonly paths: Readonly<Record<string, Readonly<Record<string, OpenApiOperation>>>>;
  readonly components?: {
    readonly schemas?: Readonly<Record<string, JsonSchemaNode>>;
    readonly securitySchemes?: Readonly<Record<string, { readonly description?: string }>>;
  };
}

/** Ordered so a family's read routes precede its writes, whatever the document's key order. */
const METHOD_ORDER = ["get", "post", "put", "patch", "delete", "head", "options", "trace"] as const;

/**
 * Section titles for the operation-id prefixes the generator uses as tags.
 *
 * Only entries a mechanical de-camel-casing gets wrong live here; anything not
 * listed is humanised by rule, so a new family needs no edit to appear.
 */
const TAG_TITLES: Readonly<Record<string, string>> = {
  adminBilling: "Admin billing",
  mcpServers: "MCP servers",
  webhook: "Webhooks",
  whoami: "Identity",
  workspace: "Workspace resources",
  workspaces: "Workspace lifecycle"
};

interface RenderedOperation {
  readonly method: string;
  readonly path: string;
  readonly operationId: string;
  readonly tag: string;
  readonly scopes: readonly string[];
  readonly requestBodySchema: string | undefined;
  readonly describesSuccessBody: boolean;
}

export function renderApiReferenceMarkdown(document: OpenApiDocument): string {
  const operations = collectOperations(document);
  const schemas = document.components?.schemas ?? {};
  const withBody = operations.filter((operation) => operation.requestBodySchema !== undefined);

  return [
    `# ${API_REFERENCE_TITLE}`,
    "",
    `Generated from \`${API_REFERENCE_SPEC_PATH}\` — the OpenAPI ${document.openapi} document that`,
    "`bun run openapi:generate` derives from the request schemas and route table in",
    "`@aexhq/contracts`. The document itself ships in the published package, so a",
    "non-TypeScript client can read this surface from the same source this page does.",
    "",
    `Version \`${document.info.version}\`. Regenerate this page with \`bun run docs:generate\`.`,
    "",
    ...renderBaseUrls(document),
    ...renderAuthentication(document, operations),
    ...renderResponses(operations, schemas),
    ...renderRoutes(operations, withBody.length),
    ...renderSchemas(schemas),
    ""
  ].join("\n");
}

function collectOperations(document: OpenApiDocument): readonly RenderedOperation[] {
  const operations: RenderedOperation[] = [];
  for (const [path, item] of Object.entries(document.paths)) {
    for (const method of METHOD_ORDER) {
      const operation = item[method];
      if (operation === undefined) continue;
      const operationId = operation.operationId ?? `${method} ${path}`;
      operations.push({
        method: method.toUpperCase(),
        path,
        operationId,
        tag: operation.tags?.[0] ?? "api",
        scopes: scopesOf(operation),
        requestBodySchema: requestBodySchemaOf(operation),
        describesSuccessBody: describesSuccessBody(operation)
      });
    }
  }
  return operations;
}

function scopesOf(operation: OpenApiOperation): readonly string[] {
  const requirement = operation.security?.[0];
  if (requirement === undefined) return [];
  return Object.values(requirement).flat();
}

function requestBodySchemaOf(operation: OpenApiOperation): string | undefined {
  const schema = operation.requestBody?.content?.["application/json"]?.schema;
  if (schema === undefined) return undefined;
  return componentNameOf(schema) ?? "inline";
}

/** A `2XX`/`200`/`default`-keyed success entry that actually carries a body schema. */
function describesSuccessBody(operation: OpenApiOperation): boolean {
  return Object.entries(operation.responses ?? {}).some(
    ([status, response]) =>
      /^(?:2\d\d|2XX)$/i.test(status) &&
      Object.values(response.content ?? {}).some((media) => media.schema !== undefined)
  );
}

function componentNameOf(schema: JsonSchemaNode): string | undefined {
  if (typeof schema.$ref !== "string") return undefined;
  return schema.$ref.split("/").at(-1);
}

function renderBaseUrls(document: OpenApiDocument): readonly string[] {
  const servers = document.servers ?? [];
  if (servers.length === 0) return [];
  return [
    "## Base URLs",
    "",
    "| Environment | Base URL |",
    "| --- | --- |",
    ...servers.map((server) => `| ${server.description ?? "default"} | \`${server.url}\` |`),
    "",
    "Every path below is relative to a base URL and already carries its `/api` prefix.",
    ""
  ];
}

function renderAuthentication(
  document: OpenApiDocument,
  operations: readonly RenderedOperation[]
): readonly string[] {
  const scheme = document.components?.securitySchemes?.workspaceApiKey;
  const unscoped = operations.filter((operation) => operation.scopes.length === 0).length;
  return [
    "## Authentication",
    "",
    scheme?.description ??
      "Workspace API key, sent as `Authorization: Bearer <key>`.",
    "",
    "The **Scope** column below names the API key scope a call requires." +
      (unscoped > 0
        ? ` ${unscoped} of ${operations.length} operations require no scope beyond a valid` +
          " workspace key and show `—`."
        : ""),
    ""
  ];
}

function renderResponses(
  operations: readonly RenderedOperation[],
  schemas: Readonly<Record<string, JsonSchemaNode>>
): readonly string[] {
  const described = operations.filter((operation) => operation.describesSuccessBody).length;
  const lines = ["## Responses", ""];

  if (described === 0) {
    // The same honesty rule as the request-body note: silence about a success
    // body must not be readable as "this route returns nothing".
    lines.push(
      "The document records that an operation succeeds without yet describing the body it",
      "returns, so no success shape appears on this page. Response schemas are being",
      "authored against the live suites; until they land, the",
      "[SDK reference](/docs/reference/sdk/) is the authority on response shapes.",
      ""
    );
  } else if (described < operations.length) {
    lines.push(
      `${described} of ${operations.length} operations describe their success body. The rest` +
        " return a body the document cannot yet describe, not an empty one.",
      ""
    );
  }

  if (schemas.ApiErrorEnvelope !== undefined) {
    lines.push(
      "A failure answers with the same JSON envelope on every operation —" +
        " [`ApiErrorEnvelope`](#apierrorenvelope) — whatever the status code." +
        " `error` is the stable machine-readable code to branch on; `message` is for humans" +
        " and may change.",
      ""
    );
  }
  return lines;
}

function renderRoutes(
  operations: readonly RenderedOperation[],
  operationsWithBody: number
): readonly string[] {
  const paths = new Set(operations.map((operation) => operation.path)).size;
  const showBodies = operationsWithBody > 0;
  const lines = [
    "## Routes",
    "",
    `${operations.length} operations across ${paths} paths.`,
    ""
  ];

  if (!showBodies) {
    // Stated rather than implied: a reader must not read a missing column as
    // "these routes take no body". The document declares no request body for
    // any operation while P2's field types are outstanding — the shapes are
    // validated by the server, they are simply not yet describable.
    lines.push(
      "None of these operations declares a request body in the document yet. Request",
      "fields are still validated by the server against schemas whose field types are",
      "being filled in; as each family lands, its operation gains a **Request body**",
      "column here linking to the component under [Schemas](#schemas). Until then use",
      "the SDK reference and the guides for request shapes.",
      ""
    );
  } else if (operationsWithBody < operations.length) {
    lines.push(
      `${operationsWithBody} of ${operations.length} operations describe their request body.` +
        " An operation showing `—` accepts a body the document cannot yet describe, not a" +
        " body it refuses.",
      ""
    );
  }

  for (const tag of [...new Set(operations.map((operation) => operation.tag))].sort()) {
    const group = operations.filter((operation) => operation.tag === tag);
    lines.push(
      `### ${titleForTag(tag)}`,
      "",
      showBodies
        ? "| Method | Path | Operation | Scope | Request body |"
        : "| Method | Path | Operation | Scope |",
      showBodies ? "| --- | --- | --- | --- | --- |" : "| --- | --- | --- | --- |",
      ...group.map((operation) => renderOperationRow(operation, showBodies)),
      ""
    );
  }
  return lines;
}

function renderOperationRow(operation: RenderedOperation, showBodies: boolean): string {
  const scope =
    operation.scopes.length === 0
      ? "—"
      : operation.scopes.map((value) => `\`${value}\``).join(", ");
  const cells = [
    `\`${operation.method}\``,
    `\`${operation.path}\``,
    `\`${operation.operationId}\``,
    scope
  ];
  if (showBodies) {
    cells.push(
      operation.requestBodySchema === undefined || operation.requestBodySchema === "inline"
        ? "—"
        : `[\`${operation.requestBodySchema}\`](#${anchor(operation.requestBodySchema)})`
    );
  }
  return `| ${cells.join(" | ")} |`;
}

function titleForTag(tag: string): string {
  const known = TAG_TITLES[tag];
  if (known !== undefined) return known;
  const spaced = tag.replace(/([a-z0-9])([A-Z])/g, "$1 $2").replace(/[._-]+/g, " ");
  return spaced.charAt(0).toUpperCase() + spaced.slice(1).toLowerCase();
}

function renderSchemas(schemas: Readonly<Record<string, JsonSchemaNode>>): readonly string[] {
  const names = Object.keys(schemas).sort();
  if (names.length === 0) return [];

  const lines = [
    "## Schemas",
    "",
    "The document's component schemas.",
    "",
    ...names.map((name) => `- [\`${name}\`](#${anchor(name)})`),
    ""
  ];

  for (const name of names) {
    const schema = schemas[name];
    if (schema === undefined) continue;
    lines.push(`### ${name}`, "");
    if (schema.description !== undefined) lines.push(schema.description, "");
    lines.push(...renderSchemaBody(schema));
  }

  if (names.some((name) => containsUndescribedValue(schemas[name]))) {
    lines.push(
      "`unknown` marks a value the document does not describe yet. The server still",
      "validates it — the type is simply not derivable until that field moves onto a",
      "schema.",
      ""
    );
  }
  return lines;
}

function renderSchemaBody(schema: JsonSchemaNode): readonly string[] {
  if (schema.type === "array") {
    const items = schema.items;
    // An inline item object carries the whole shape; rendering it as `object[]`
    // would drop every field the document does describe.
    if (items !== undefined && hasProperties(items)) {
      return ["An array. Each item:", "", ...renderSchemaBody(items)];
    }
    return [`An array of ${renderType(items)}.`, ""];
  }
  if (!hasProperties(schema)) {
    return [`${renderType(schema)}.`, ""];
  }
  const required = new Set(schema.required ?? []);
  const lines = [
    "| Property | Type | Required | Notes |",
    "| --- | --- | --- | --- |",
    ...Object.entries(schema.properties ?? {}).map(
      ([property, node]) =>
        `| \`${property}\` | ${renderType(node)} | ${required.has(property) ? "yes" : "no"} | ${
          notesFor(node) || "—"
        } |`
    ),
    ""
  ];
  if (schema.additionalProperties === false) {
    lines.push("Unknown fields are rejected.", "");
  } else if (schema.additionalProperties === true) {
    lines.push("Additional fields may appear.", "");
  }
  return lines;
}

/** A compact TypeScript-shaped rendering, escaped for a GFM table cell. */
export function renderType(node: JsonSchemaNode | undefined): string {
  const referenced = node === undefined ? undefined : componentNameOf(node);
  if (referenced !== undefined) return `[\`${referenced}\`](#${anchor(referenced)})`;
  return `\`${escapeCell(typeExpression(node))}\``;
}

function typeExpression(node: JsonSchemaNode | undefined): string {
  if (node === undefined) return "unknown";
  const referenced = componentNameOf(node);
  if (referenced !== undefined) return referenced;
  if (node.const !== undefined) return JSON.stringify(node.const);
  if (node.enum !== undefined) return node.enum.map((value) => JSON.stringify(value)).join(" | ");
  const composed = node.anyOf ?? node.oneOf ?? node.allOf;
  if (composed !== undefined) return composed.map(typeExpression).join(" | ");
  const type = node.type;
  if (Array.isArray(type)) return type.join(" | ");
  if (type === "array") return `${typeExpression(node.items)}[]`;
  if (type === "object") return objectExpression(node);
  if (typeof type === "string") return type;
  return "unknown";
}

function objectExpression(node: JsonSchemaNode): string {
  const additional = node.additionalProperties;
  if (hasProperties(node)) return "object";
  if (typeof additional === "object") {
    return `Record<string, ${typeExpression(isEmptySchema(additional) ? undefined : additional)}>`;
  }
  if (additional === true) return "Record<string, unknown>";
  return "object";
}

function isEmptySchema(node: JsonSchemaNode): boolean {
  return Object.keys(node).length === 0;
}

function hasProperties(node: JsonSchemaNode): boolean {
  return node.properties !== undefined && Object.keys(node.properties).length > 0;
}

/**
 * Whether anything under this schema renders as `unknown` — the document's way
 * of saying a value's type is not derivable yet. Drives one explanatory
 * sentence, so it is deliberately computed from the same `typeExpression` the
 * tables print rather than by grepping the rendered output.
 */
function containsUndescribedValue(node: JsonSchemaNode | undefined): boolean {
  if (node === undefined) return true;
  if (typeExpression(node) === "unknown") return true;
  if (node.$ref !== undefined) return false;
  const children = [
    ...(node.items === undefined ? [] : [node.items]),
    ...Object.values(node.properties ?? {}),
    ...(typeof node.additionalProperties === "object" ? [node.additionalProperties] : []),
    ...(node.anyOf ?? []),
    ...(node.oneOf ?? []),
    ...(node.allOf ?? [])
  ];
  return children.some(containsUndescribedValue);
}

function notesFor(node: JsonSchemaNode): string {
  const notes: string[] = [];
  if (node.description !== undefined) notes.push(node.description);
  if (node.format !== undefined) notes.push(`format \`${node.format}\``);
  if (node.pattern !== undefined) notes.push(`matches \`${escapeCell(node.pattern)}\``);
  if (node.minLength !== undefined) notes.push(`at least ${plural(node.minLength, "character")}`);
  if (node.maxLength !== undefined) notes.push(`at most ${plural(node.maxLength, "character")}`);
  if (node.minimum !== undefined) notes.push(`minimum ${node.minimum}`);
  if (node.maximum !== undefined) notes.push(`maximum ${node.maximum}`);
  if (node.minItems !== undefined) notes.push(`at least ${plural(node.minItems, "item")}`);
  if (node.maxItems !== undefined) notes.push(`at most ${plural(node.maxItems, "item")}`);
  return notes.join("; ");
}

function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

/**
 * A literal `|` ends a GFM table cell even inside a code span, so it is escaped;
 * a newline would end the row outright, so it is folded.
 */
function escapeCell(value: string): string {
  return value.replace(/\|/g, "\\|").replace(/\s*\n\s*/g, " ");
}

/** GitHub/fumadocs heading slug for the component headings rendered above. */
export function anchor(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9 -]/g, "")
    .replace(/\s+/g, "-");
}
