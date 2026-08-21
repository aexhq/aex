import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import YAML from "yaml";

const root = path.resolve(import.meta.dirname, "..");
const brainOpenApiPath = path.resolve(root, "../brain/contracts/session/v1/openapi.yaml");
const overlayPath = path.join(root, "contracts/hosted/brain-session.overlay.yaml");

const fail = (message) => {
  throw new Error(message);
};
const contractBytes = (bytes) => {
  const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  const normalized = text.replaceAll("\r\n", "\n");
  if (normalized.includes("\r")) fail("Brain OpenAPI contains a non-CRLF carriage return");
  return Buffer.from(normalized, "utf8");
};
const contractDigest = (bytes) => createHash("sha256").update(contractBytes(bytes)).digest("hex");
const asObject = (value, label) => {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail(`${label} must be an object`);
  }
  return value;
};

const [brainBytes, overlayBytes, cargoBytes] = await Promise.all([
  readFile(brainOpenApiPath),
  readFile(overlayPath),
  readFile(path.join(root, "Cargo.toml")),
]);
const brain = asObject(YAML.parse(brainBytes.toString("utf8")), "Brain OpenAPI");
const overlay = asObject(YAML.parse(overlayBytes.toString("utf8")), "Aex hosted overlay");

if (overlay.overlay !== "1.0.0" || overlay.info?.version !== "mvp") {
  fail("the hosted policy must remain one MVP Overlay 1.0 document");
}
const revision = /^https:\/\/raw\.githubusercontent\.com\/aexhq\/brain\/([0-9a-f]{40})\/contracts\/session\/v1\/openapi\.yaml$/u
  .exec(overlay.extends ?? "")?.[1];
if (revision === undefined) fail("the hosted overlay must extend one immutable Brain revision");
const cargo = cargoBytes.toString("utf8");
const dependencyNames = ["brain-protocol", "brain", "brain-aws", "brain-standalone"];
const dependencyRevisions = dependencyNames.map((name) => {
  const match = cargo.match(
    new RegExp(`^${name}\\s*=\\s*\\{[^\\n]*\\brev\\s*=\\s*"([0-9a-f]{40})"`, "mu"),
  );
  if (match === null) fail(`${name} must pin one immutable Brain revision`);
  return match[1];
});
if (dependencyRevisions.some((dependencyRevision) => dependencyRevision !== revision)) {
  fail("the hosted overlay and all Rust Brain dependencies must pin the same revision");
}

const digest = contractDigest(brainBytes);
if (overlay["x-aex-brain-openapi-sha256"] !== digest) {
  fail(`Brain OpenAPI digest drifted: expected ${overlay["x-aex-brain-openapi-sha256"]}, got ${digest}`);
}
// Git stores and raw.githubusercontent.com serves LF bytes, while a Windows worktree may check out
// the same blob as CRLF. The hosted identity is the LF Git-blob digest, never a platform checkout
// digest. Keep this executable regression beside the verifier so both representations prove the
// same one-way identity without accepting two hashes.
const lfFixture = contractBytes(brainBytes);
const crlfFixture = Buffer.from(lfFixture.toString("utf8").replaceAll("\n", "\r\n"), "utf8");
if (contractDigest(lfFixture) !== contractDigest(crlfFixture)) {
  fail("Brain OpenAPI digest must be stable across LF and CRLF worktrees");
}

const paths = asObject(brain.paths, "Brain paths");
const identityParameter = asObject(
  asObject(asObject(brain.components, "Brain components").parameters, "Brain parameters")
    .IdempotencyKey,
  "Brain IdempotencyKey parameter",
);
if (
  identityParameter.name !== "Idempotency-Key" ||
  identityParameter.in !== "header" ||
  identityParameter.required !== false ||
  identityParameter.schema?.maxLength !== 128
) {
  fail("Brain's optional IdempotencyKey component drifted before the hosted policy was applied");
}
const expectedOptionalIdentityTargets = [
  ["/v1/sessions", "post"],
  ["/v1/sessions/{session_id}/messages", "post"],
  ["/v1/sessions/{session_id}/children", "post"],
  ["/v1/sessions/{session_id}/children/{child_id}/messages", undefined],
  ["/v1/sessions/{session_id}/children/{child_id}/follow-up", undefined],
];
for (const [pathname, method] of expectedOptionalIdentityTargets) {
  const pathItem = asObject(paths[pathname], `Brain path ${pathname}`);
  const owner = method === undefined
    ? pathItem
    : asObject(pathItem[method], `Brain operation ${method.toUpperCase()} ${pathname}`);
  if (
    !Array.isArray(owner.parameters) ||
    !owner.parameters.some((parameter) => parameter?.$ref === "#/components/parameters/IdempotencyKey")
  ) {
    fail(`hosted idempotency target disappeared from ${method?.toUpperCase() ?? "path"} ${pathname}`);
  }
}

const create = asObject(
  asObject(paths["/v1/sessions"], "Brain sessions path").post,
  "Brain create operation",
);
if (create.operationId !== "createSession") fail("Brain createSession operation drifted");
const requestSchema = create.requestBody?.content?.["application/json"]?.schema?.$ref;
if (requestSchema !== "schemas.json#/$defs/CreateSessionRequest") {
  fail("Brain createSession request schema target drifted");
}

if (!Array.isArray(overlay.actions) || overlay.actions.length !== 2) {
  fail("the hosted overlay must contain exactly the two narrow MVP policy actions");
}
const identityAction = asObject(overlay.actions[0], "hosted idempotency action");
const identityUpdate = asObject(identityAction.update, "hosted idempotency update");
if (
  identityAction.target !== "$.components.parameters.IdempotencyKey" ||
  identityUpdate.required !== true ||
  identityUpdate.schema?.minLength !== 1 ||
  identityUpdate.schema?.maxLength !== 128
) {
  fail("the hosted overlay no longer requires the bounded Idempotency-Key component");
}
const shapeAction = asObject(overlay.actions[1], "hosted shape action");
if (
  shapeAction.target !== "$['paths']['/v1/sessions']['post']" ||
  shapeAction.update?.["x-aex-managed-compute"]?.shape !== "1gb" ||
  shapeAction.update?.["x-aex-managed-compute"]?.selectable !== false
) {
  fail("the hosted overlay no longer records the fixed 1gb create policy");
}

process.stdout.write(`verified hosted overlay against Brain ${revision} (${digest})\n`);
