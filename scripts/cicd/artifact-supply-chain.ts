import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
type ObjectJson = { [key: string]: Json };

const sha256 = (bytes: string | Buffer): string =>
  `sha256:${createHash("sha256").update(bytes).digest("hex")}`;

const object = (value: Json | undefined, label: string): ObjectJson => {
  if (value === null || Array.isArray(value) || typeof value !== "object") {
    throw new Error(`${label} must be an object`);
  }
  return value;
};

const array = (value: Json | undefined, label: string): Json[] => {
  if (!Array.isArray(value)) throw new Error(`${label} must be an array`);
  return value;
};

const string = (value: Json | undefined, label: string): string => {
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${label} must be a non-empty string`);
  }
  return value;
};

const canonical = (value: Json): string => `${JSON.stringify(value, null, 2)}\n`;

type DenyPolicy = {
  readonly licenses?: {
    readonly allow?: readonly string[];
    readonly clarify?: readonly { readonly crate?: string; readonly expression?: string }[];
    readonly exceptions?: readonly { readonly crate?: string; readonly allow?: readonly string[] }[];
  };
};

const licensesOf = (component: ObjectJson): string[] => {
  const declared = array(component.licenses, `component ${String(component.name)} licenses`);
  return declared.map((entry, index) => {
    const item = object(entry, `license ${index}`);
    if (typeof item.expression === "string" && item.expression.length > 0) return item.expression;
    const license = object(item.license, `license ${index}.license`);
    return string(license.id, `license ${index}.license.id`);
  });
};

const expressionAllowed = (
  component: string,
  expression: string,
  allowed: ReadonlySet<string>,
  clarified: ReadonlyMap<string, string>
): boolean => {
  if (allowed.has(expression) || clarified.get(component) === expression) return true;
  return expression
    .replaceAll("(", "")
    .replaceAll(")", "")
    .split(/\s+(?:AND|OR)\s+/u)
    .every((part) => allowed.has(part.trim()));
};

const exactPackageKey = (value: string): string => {
  const separator = value.lastIndexOf("@");
  if (separator <= 0 || separator === value.length - 1) {
    throw new Error(`license exception ${value} must pin an exact crate version`);
  }
  const version = value.slice(separator + 1);
  if (/[<>=*^~,\s]/u.test(version)) {
    throw new Error(`license exception ${value} must pin an exact crate version`);
  }
  return value;
};

export type SupplyChainOutputs = {
  readonly sbom: ObjectJson;
  readonly licenseInventory: ObjectJson;
  readonly vulnerabilityVerdict: ObjectJson;
  readonly claims: ObjectJson;
  readonly reports: Readonly<Record<"sbom" | "license" | "vulnerability", ObjectJson>>;
};

export const inspectSupplyChain = (inputs: {
  readonly rawSbom: ObjectJson;
  readonly draft: ObjectJson;
  readonly grype: ObjectJson;
  readonly denyPolicy: DenyPolicy;
  readonly denyPolicyBytes: string;
  readonly syftVersion: string;
  readonly grypeVersion: string;
  readonly scannedAt: string;
}): SupplyChainOutputs => {
  const subject = string(inputs.draft.artifactSubjectDigest, "draft artifactSubjectDigest");
  const unit = string(object(inputs.draft.unit, "draft unit").id, "draft unit id");
  if (inputs.rawSbom.bomFormat !== "CycloneDX" || inputs.rawSbom.specVersion !== "1.6") {
    throw new Error("Syft must emit CycloneDX 1.6");
  }
  const metadata = object(inputs.rawSbom.metadata ?? {}, "SBOM metadata");
  const subjectComponent = metadata.component === undefined
    ? null
    : object(metadata.component, "SBOM metadata component");
  const detectedComponents = inputs.rawSbom.components === undefined
    ? []
    : array(inputs.rawSbom.components, "SBOM components").map((entry, index) =>
        object(entry, `component ${index}`)
      );
  const components = [
    ...(subjectComponent === null
      ? []
      : [{ component: subjectComponent, source: "metadata.component" as const }]),
    ...detectedComponents.map((component) => ({ component, source: "components" as const }))
  ];
  if (components.length === 0) throw new Error("the exact artifact SBOM is empty");

  const properties = Array.isArray(metadata.properties) ? metadata.properties : [];
  metadata.properties = [
    ...properties.filter(
      (entry) => object(entry, "SBOM property").name !== "aex:artifactSubjectDigest"
    ),
    { name: "aex:artifactSubjectDigest", value: subject }
  ];
  const sbom: ObjectJson = { ...inputs.rawSbom, metadata };

  const allowed = new Set(inputs.denyPolicy.licenses?.allow ?? []);
  if (allowed.size === 0) throw new Error("deny.toml has no license allowlist");
  const clarified = new Map(
    (inputs.denyPolicy.licenses?.clarify ?? []).map((entry) => [
      string(entry.crate, "clarified crate"),
      string(entry.expression, "clarified expression")
    ])
  );
  const exceptions = new Map<string, Set<string>>();
  for (const entry of inputs.denyPolicy.licenses?.exceptions ?? []) {
    const key = exactPackageKey(string(entry.crate, "license exception crate"));
    const licenses = new Set(
      (entry.allow ?? []).map((license) => string(license, `license exception ${key}`))
    );
    if (licenses.size === 0) throw new Error(`license exception ${key} has no allow list`);
    exceptions.set(key, licenses);
  }
  const inventory = components.map(({ component, source }) => {
    const name = string(component.name, "component name");
    const version = typeof component.version === "string" ? component.version : null;
    const licenseEvidence = component.licenses === undefined ? "not-declared" : "declared";
    if (source === "components" && licenseEvidence === "not-declared") {
      throw new Error(`component ${name} licenses must be an array`);
    }
    const licenses = licenseEvidence === "declared" ? licensesOf(component) : [];
    const componentAllowed = new Set(allowed);
    if (version !== null) {
      for (const license of exceptions.get(`${name}@${version}`) ?? []) {
        componentAllowed.add(license);
      }
    }
    const denied = licenses.filter(
      (expression) => !expressionAllowed(name, expression, componentAllowed, clarified)
    );
    return { source, name, version, licenseEvidence, licenses, denied };
  });
  const denials = inventory.flatMap((component) =>
    component.denied.map((license) => `${component.name}: ${license}`)
  );
  if (denials.length > 0) {
    throw new Error(`license policy denied ${denials.join(", ")}`);
  }
  const policyDigest = sha256(inputs.denyPolicyBytes);
  const licenseInventory: ObjectJson = {
    schema: "aex.license-inventory.v1",
    unit,
    artifactSubjectDigest: subject,
    policyDigest,
    components: inventory as unknown as Json
  };

  const matches = array(inputs.grype.matches, "Grype matches").map((entry, index) =>
    object(entry, `Grype match ${index}`)
  );
  const severities = matches.map((match, index) =>
    string(object(match.vulnerability, `Grype match ${index} vulnerability`).severity,
      `Grype match ${index} severity`).toLowerCase()
  );
  const critical = severities.filter((severity) => severity === "critical").length;
  const high = severities.filter((severity) => severity === "high").length;
  if (critical > 0 || high > 0) {
    throw new Error(`Grype found ${critical} critical and ${high} high vulnerabilities`);
  }
  const descriptor = object(inputs.grype.descriptor, "Grype descriptor");
  const database = object(descriptor.db, "Grype database descriptor");
  const databaseIdentity = sha256(canonical(database));
  const vulnerabilityVerdict: ObjectJson = {
    schema: "aex.vulnerability-verdict.v1",
    unit,
    artifactSubjectDigest: subject,
    scanner: `grype ${inputs.grypeVersion}`,
    database: databaseIdentity,
    scannedAt: inputs.scannedAt,
    unapprovedCritical: critical,
    unapprovedHigh: high,
    matches: matches.length
  };
  const report = (producer: string, checks: string[]): ObjectJson => ({
    schema: "aex.check-report.v1",
    producer,
    checks: checks.map((id) => ({ id, status: "passed" }))
  });
  return {
    sbom,
    licenseInventory,
    vulnerabilityVerdict,
    claims: {
      licenses: { policyDigest, verdict: "allowed", denials: [] },
      vulnerabilities: {
        scanner: `grype ${inputs.grypeVersion}`,
        database: databaseIdentity,
        scannedAt: inputs.scannedAt,
        unapprovedCritical: critical,
        unapprovedHigh: high,
        approvedExceptions: []
      }
    },
    reports: {
      sbom: report(`syft ${inputs.syftVersion}`, [
        "cyclonedx-1.6-nonempty",
        "artifact-subject-bound"
      ]),
      license: report("artifact-supply-chain.ts + cargo-deny 0.20.2", [
        "complete-component-inventory",
        "deny-policy-allowed"
      ]),
      vulnerability: report(`grype ${inputs.grypeVersion}`, [
        "database-identified",
        "no-unapproved-high-or-critical"
      ])
    }
  };
};

const args = new Map<string, string>();
for (let index = 2; index < process.argv.length; index += 2) {
  const key = process.argv[index];
  const value = process.argv[index + 1];
  if (!key?.startsWith("--") || value === undefined) throw new Error("arguments are --name value");
  args.set(key.slice(2), value);
}
if (import.meta.main) {
  const required = (name: string): string => {
    const value = args.get(name);
    if (!value) throw new Error(`missing --${name}`);
    return value;
  };
  const readJson = (name: string): ObjectJson =>
    object(JSON.parse(readFileSync(resolve(required(name)), "utf8")) as Json, name);
  const policyPath = resolve(required("deny-policy"));
  const policyBytes = readFileSync(policyPath, "utf8");
  const outputs = inspectSupplyChain({
    rawSbom: readJson("raw-sbom"),
    draft: readJson("draft"),
    grype: readJson("grype"),
    denyPolicy: Bun.TOML.parse(policyBytes) as DenyPolicy,
    denyPolicyBytes: policyBytes,
    syftVersion: required("syft-version"),
    grypeVersion: required("grype-version"),
    scannedAt: required("scanned-at")
  });
  const out = resolve(required("out-dir"));
  mkdirSync(out, { recursive: true });
  const write = (name: string, value: Json): void =>
    writeFileSync(resolve(out, name), canonical(value));
  write("sbom.cdx.json", outputs.sbom);
  write("license-inventory.json", outputs.licenseInventory);
  write("vulnerability-verdict.json", outputs.vulnerabilityVerdict);
  write("supply-chain-claims.json", outputs.claims);
  for (const [name, report] of Object.entries(outputs.reports)) {
    write(`report-${name}.json`, report);
  }
}
