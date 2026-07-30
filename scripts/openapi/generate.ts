#!/usr/bin/env bun
/**
 * P4 — emit the OpenAPI documents from the schemas and the route table.
 *
 * The document is GENERATED, never authored. It is committed so a reader can
 * diff it, and `--check` fails when the committed copy and a fresh generation
 * disagree — the same generate/check pair as `capabilities:generate` /
 * `capabilities:check`.
 *
 * This imports **full `zod`**, not `zod/mini`: `z.toJSONSchema` is tree-shaken
 * out of the mini entrypoint (measured — `~standard.jsonSchema` is absent on
 * mini schemas). Both entrypoints construct the same core classes, so the
 * conversion reads the very objects the server validates with. That is the whole
 * point: there is no second description of the wire to keep in sync.
 *
 * Emits **openapi-3.1**, which is JSON-Schema-compatible. 3.0 renders `z.null()`
 * as `{"type":"string","nullable":true,"enum":[null]}` (measured) and is only
 * worth producing if a downstream consumer demands it.
 *
 * Usage:
 *   bun scripts/openapi/generate.ts            # write
 *   bun scripts/openapi/generate.ts --check    # fail if committed output is stale
 */
import { readFileSync } from "node:fs";
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { z } from "zod";
import {
  BOOTSTRAP_API_ROUTE_DESCRIPTORS,
  REGIONAL_API_ROUTE_DESCRIPTORS,
  type AuthenticatedApiRouteDescriptor
} from "../../packages/contracts/src/api-routes.js";
import { OPENAPI_SCHEMA_REGISTRY, OPENAPI_REQUEST_BODIES } from "./registry.js";

const repoRoot = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const outputDir = resolve(repoRoot, "packages/contracts/openapi");

/** The two planes have different hosts and different principals, so two documents. */
interface PlaneSpec {
  readonly file: string;
  readonly title: string;
  readonly description: string;
  readonly servers: readonly { readonly url: string; readonly description: string }[];
  readonly routes: readonly AuthenticatedApiRouteDescriptor[];
  readonly securityScheme: string;
  readonly securityDescription: string;
}

/**
 * Regex syntax that cannot occur in a path segment we would emit literally.
 *
 * A segment carrying any of it is a CONSTRAINED variable —
 * `[0-9]{4}-(0[1-9]|1[0-2])` on `/billing/statements/…`, which admits a UTC
 * month and nothing else. Those exist because refusing a malformed id at the
 * route table is stronger than routing it into a handler to reject, and the
 * table would lose that power if the only variable form this generator knew were
 * the wide-open `[^/]+`. OpenAPI cannot express "this segment matches that
 * regex" on a path template, so the template names the parameter and the route
 * table keeps the constraint.
 */
const CONSTRAINED_SEGMENT = /[[\]{}()|+*?]/;

/**
 * Turn a dispatch RegExp into an OpenAPI path template.
 *
 * `/^\/sessions\/[^/]+\/messages$/` -> `/sessions/{sessionId}/messages`. A
 * variable segment is named from the collection that precedes it, which is how
 * the handlers already name them (`sessionId`, `fileId`, `deliveryId`).
 */
export function toPathTemplate(pattern: RegExp): {
  readonly path: string;
  readonly parameters: readonly string[];
} {
  // Collapse the unconstrained variable segment to a marker BEFORE splitting:
  // `[^/]+` contains a literal `/` inside its character class, so splitting first
  // tears it in two and yields paths like `/assets/{assetId}/]+`.
  const VARIABLE = "__PARAM__";
  const source = pattern.source
    .replace(/^\^/, "")
    .replace(/\$$/, "")
    .replace(/\[\^\\?\/\]\+/g, VARIABLE)
    .replace(/\\\//g, "/");
  const parameters: string[] = [];
  const segments = source.split("/").filter((segment) => segment.length > 0);
  const rendered = segments.map((segment, index) => {
    if (segment !== VARIABLE && !CONSTRAINED_SEGMENT.test(segment)) {
      return segment;
    }
    const name = parameterName(segments[index - 1], parameters.length);
    parameters.push(name);
    return `{${name}}`;
  });
  return { path: `/${rendered.join("/")}`, parameters };
}

function parameterName(precedingSegment: string | undefined, position: number): string {
  if (precedingSegment === undefined || precedingSegment.includes("__PARAM__")) {
    return `param${position + 1}`;
  }
  const singular = precedingSegment
    .replace(/ies$/, "y")
    .replace(/s$/, "")
    .replace(/-([a-z])/g, (_match, letter: string) => letter.toUpperCase());
  return `${singular}Id`;
}

function operationFor(
  plane: PlaneSpec,
  descriptor: AuthenticatedApiRouteDescriptor,
  parameters: readonly string[]
) {
  const requestBody = OPENAPI_REQUEST_BODIES[descriptor.name];
  const idempotencyParameter =
    descriptor.idempotency === "operation-id"
      ? {
          name: "Aex-Operation-Id",
          in: "header" as const,
          required: true,
          schema: { type: "string" as const, pattern: "^op_[0-9a-hjkmnp-tv-z]{26}$" }
        }
      : descriptor.idempotency === "idempotency-key"
        ? {
            name: "Idempotency-Key",
            in: "header" as const,
            required: true,
            schema: { type: "string" as const, minLength: 1, maxLength: 255 }
          }
        : undefined;
  const allParameters = [
    ...parameters.map((name) => ({
      name,
      in: "path" as const,
      required: true,
      schema: { type: "string" as const }
    })),
    ...(idempotencyParameter ? [idempotencyParameter] : [])
  ];
  return {
    operationId: descriptor.name,
    summary: descriptor.name,
    tags: [descriptor.name.split(".")[0] ?? "api"],
    ...(allParameters.length > 0 ? { parameters: allParameters } : {}),
    ...(requestBody
      ? {
          requestBody: {
            required: true,
            content: {
              "application/json": {
                schema: { $ref: `#/components/schemas/${requestBody}` }
              }
            }
          }
        }
      : {}),
    // `requiredScope` is route metadata today and becomes Hono route metadata at
    // P5, at which point this stops being a projection and starts being the
    // declaration itself.
    security: [{ [plane.securityScheme]: descriptor.requiredScope ? [descriptor.requiredScope] : [] }],
    responses: {
      "2XX": { description: "Success." },
      default: {
        description: "Error envelope.",
        content: {
          "application/json": {
            schema: { $ref: "#/components/schemas/ApiError" }
          }
        }
      }
    }
  };
}

function buildDocument(plane: PlaneSpec): unknown {
  const paths: Record<string, Record<string, unknown>> = {};
  for (const descriptor of plane.routes) {
    const { path, parameters } = toPathTemplate(descriptor.pattern);
    const key = `/api${path}`;
    paths[key] ??= {};
    paths[key][descriptor.method.toLowerCase()] = operationFor(plane, descriptor, parameters);
  }

  // One conversion of the whole registry, so a schema reused by two operations
  // is emitted once and referenced twice.
  const converted = z.toJSONSchema(OPENAPI_SCHEMA_REGISTRY, {
    target: "draft-2020-12",
    io: "input",
    uri: (id) => `#/components/schemas/${id}`
  }) as { schemas: Record<string, Record<string, unknown>> };

  const schemas: Record<string, unknown> = {};
  for (const [id, schema] of Object.entries(converted.schemas)) {
    // `$schema` and `$id` are JSON Schema document furniture; inside an OpenAPI
    // components map they are noise.
    const { $schema: _schema, $id: _id, ...rest } = schema;
    schemas[id] = rest;
  }
  return {
    openapi: "3.1.0",
    info: {
      title: plane.title,
      version: readVersion(),
      description: plane.description
    },
    servers: plane.servers,
    paths: sortKeys(paths),
    components: {
      securitySchemes: {
        [plane.securityScheme]: {
          type: "http",
          scheme: "bearer",
          description: plane.securityDescription
        }
      },
      schemas: sortKeys(schemas)
    }
  };
}

function readVersion(): string {
  return (
    JSON.parse(
      readFileSync(resolve(repoRoot, "packages/contracts/package.json"), "utf8")
    ) as { version: string }
  ).version;
}

function normalizeEol(text: string): string {
  return text.replace(/\r\n/g, "\n");
}

function sortKeys<T>(value: Record<string, T>): Record<string, T> {
  return Object.fromEntries(Object.entries(value).sort(([left], [right]) => left.localeCompare(right)));
}

const PLANES: readonly PlaneSpec[] = [
  {
    file: "bootstrap.json",
    title: "aex bootstrap API",
    description:
      "The global account, organization, workspace-placement, key, and billing API. " +
      "Generated from strict v1 contract schemas and src/api-routes.ts — do not edit by hand.",
    servers: [
      { url: "https://api.aex.dev", description: "bootstrap" }
    ],
    routes: BOOTSTRAP_API_ROUTE_DESCRIPTORS,
    securityScheme: "accountToken",
    securityDescription: "Account identity token, sent as `Authorization: Bearer <token>`."
  },
  {
    file: "regional.json",
    title: "aex regional workspace API",
    description:
      "The region-pinned workspace and session API. Generated from " +
      "strict v1 contract schemas and src/api-routes.ts — do not edit by hand.",
    servers: [
      { url: "https://us-east-1.api.aex.dev", description: "us-east-1" },
      { url: "https://us-east-2.api.aex.dev", description: "us-east-2" },
      { url: "https://us-west-2.api.aex.dev", description: "us-west-2" },
      { url: "https://ap-northeast-1.api.aex.dev", description: "ap-northeast-1" },
      { url: "https://eu-west-1.api.aex.dev", description: "eu-west-1" }
    ],
    routes: REGIONAL_API_ROUTE_DESCRIPTORS,
    securityScheme: "workspaceApiKey",
    securityDescription: "Region-pinned workspace API key, sent as `Authorization: Bearer <key>`."
  }
];

/**
 * G1 — every route in the table appears in the document.
 *
 * Still a real gate at P4: the routes and the spec are two artefacts derived
 * from one table, and a bug in the pattern-to-template conversion could drop or
 * merge one. P5 dissolves it — under `@hono/zod-openapi` a route and its spec
 * entry are the same declaration, and it stops being expressible for them to
 * disagree.
 */
function assertEveryRouteIsDocumented(plane: PlaneSpec, document: unknown): void {
  const paths = (document as { paths: Record<string, Record<string, unknown>> }).paths;
  const documented = new Set<string>();
  for (const [path, operations] of Object.entries(paths)) {
    for (const [method, operation] of Object.entries(operations)) {
      void path;
      void method;
      documented.add((operation as { operationId: string }).operationId);
    }
  }
  const missing = plane.routes
    .map((route) => route.name)
    .filter((name) => !documented.has(name));
  if (missing.length > 0) {
    throw new Error(
      `openapi: ${missing.length} route(s) in the table produced no operation: ${missing.join(", ")}`
    );
  }
  const duplicates = plane.routes.length - documented.size;
  if (duplicates > 0) {
    throw new Error(
      `openapi: ${duplicates} route(s) collapsed onto an existing method+path — ` +
        "two routes rendered to the same template."
    );
  }
}

async function main(): Promise<number> {
  const check = process.argv.includes("--check");
  await mkdir(outputDir, { recursive: true });

  let stale = false;
  for (const plane of PLANES) {
    const document = buildDocument(plane);
    assertEveryRouteIsDocumented(plane, document);
    const rendered = `${JSON.stringify(document, null, 2)}\n`;
    const target = resolve(outputDir, plane.file);
    if (check) {
      const existing = await readFile(target, "utf8").catch(() => "");
      // Compare with line endings normalised. `.gitattributes` pins these files
      // to LF so the working tree is canonical, but a checkout made before that
      // landed — or with a local core.autocrlf — would otherwise report a clean
      // tree as stale, which teaches people to ignore the gate.
      if (normalizeEol(existing) !== normalizeEol(rendered)) {
        console.error(
          `openapi:check FAILED — ${plane.file} is stale. Run \`bun run openapi:generate\`.`
        );
        stale = true;
      }
      continue;
    }
    await writeFile(target, rendered, "utf8");
    console.log(`openapi: wrote ${plane.file}`);
  }

  if (check && !stale) {
    console.log(`openapi:check OK: ${PLANES.length} document(s) match the schemas.`);
  }
  return stale ? 1 : 0;
}

if (import.meta.main) {
  process.exit(await main());
}
