import { expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

const schemas = resolve(import.meta.dir, "../../..", "api/generated/schemas");
const source = readFileSync(resolve(import.meta.dir, "../src/ui/wire.ts"), "utf8");

/**
 * The dashboard cannot invent a field.
 *
 * `src/ui/wire.ts` is a hand-written stand-in for the generated payload types. This
 * test reads it back and checks every declared field against the checked-in JSON
 * schema: a name that is not a property of the schema fails, and a field declared
 * non-optional that the schema does not require fails. It is what keeps a
 * placeholder declaration from quietly becoming a fabricated API.
 */
const SCHEMA_FOR: Readonly<Record<string, string>> = {
  SessionListItem: "SessionListItem",
  Session: "Session",
  Message: "Message",
  MissingInterval: "MissingInterval",
  ObservationCoverage: "ObservationCoverage",
  Observation: "Observation",
  ObservationPage: "ObservationPage",
  TraceDetail: "TraceDetail",
  TelemetryGap: "TelemetryGap",
  UsageFrontier: "UsageFrontier",
  UsageAttribution: "UsageAttribution",
  UsagePage: "UsagePage",
  BillingBalance: "BillingBalance",
  AutoTopupPolicy: "AutoTopupPolicy",
  StatementSummary: "StatementSummary",
  HostedSession: "HostedSession",
  DownloadGrant: "DownloadGrant",
  ApiKey: "ApiKey",
  NewApiKey: "NewApiKey",
  ProviderCredential: "ProviderCredential",
  RegisteredEntry: "RegisteredFile",
  EffectiveWorkspaceLimit: "EffectiveWorkspaceLimit",
  FileEntry: "FileEntry",
  LiveFileEntryPage: "LiveFileEntryPage",
};

interface Field {
  readonly name: string;
  readonly optional: boolean;
}

/** Top-level `readonly name?:` declarations of one interface block. */
function fieldsOf(name: string): readonly Field[] {
  const header = new RegExp(`export interface ${name}(?: extends \\w+)? \\{`).exec(source);
  if (!header) throw new Error(`wire.ts declares no interface ${name}`);
  let depth = 0;
  const fields: Field[] = [];
  let line = "";
  for (let index = header.index + header[0].length - 1; index < source.length; index += 1) {
    const character = source[index];
    if (character === "{") depth += 1;
    if (character === "}") {
      depth -= 1;
      if (depth === 0) break;
    }
    if (character === "\n") {
      const match = /^\s*readonly ([A-Za-z0-9_]+)(\?)?:/.exec(line);
      if (match?.[1] && depth === 1) fields.push({ name: match[1], optional: match[2] === "?" });
      line = "";
      continue;
    }
    line += character;
  }
  return fields;
}

test("every declared payload type maps to a checked-in schema", () => {
  const declared = [...source.matchAll(/export interface ([A-Za-z0-9_]+)/g)].map((match) => match[1]!);
  const unmapped = declared.filter((name) => name !== "Page" && !(name in SCHEMA_FOR));
  expect(unmapped).toEqual([]);
});

test("no declared field is absent from its schema, and no optional field is required", () => {
  for (const [declaredName, schemaName] of Object.entries(SCHEMA_FOR)) {
    const path = resolve(schemas, `${schemaName}.json`);
    expect({ schemaName, exists: existsSync(path) }).toEqual({ schemaName, exists: true });
    const schema = JSON.parse(readFileSync(path, "utf8")) as {
      properties?: Record<string, unknown>;
      required?: string[];
    };
    const properties = new Set(Object.keys(schema.properties ?? {}));
    const required = new Set(schema.required ?? []);
    const fields = fieldsOf(declaredName);
    expect({ declaredName, count: fields.length > 0 }).toEqual({ declaredName, count: true });
    for (const field of fields) {
      expect({ declaredName, field: field.name, known: properties.has(field.name) })
        .toEqual({ declaredName, field: field.name, known: true });
      // A field declared without `?` claims the API always sends it, so the schema
      // must require it. An optional declaration makes no claim either way.
      const claimSupported = field.optional || required.has(field.name);
      expect({ declaredName, field: field.name, nonOptionalIsRequired: claimSupported })
        .toEqual({ declaredName, field: field.name, nonOptionalIsRequired: true });
    }
  }
});
