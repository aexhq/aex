import { CANONICAL_SHA256_DIGEST_PATTERN } from "../../packages/contracts/src/canonical-sha256.ts";

const RUNTIME_KINDS = ["container", "spot_container", "lambda"];
const RUNTIME_SIZES = [
  "0.25cpu-1gb",
  "0.5cpu-4gb",
  "1cpu-6gb",
  "2cpu-8gb",
  "4cpu-12gb"
];
const LEGACY_RUNTIME_SIZE_ALIASES = {
  "shared-0.25x-1gb": "0.25cpu-1gb",
  "shared-0.5x-4gb": "0.5cpu-4gb",
  "shared-1x-6gb": "1cpu-6gb",
  "shared-2x-8gb": "2cpu-8gb",
  "shared-4x-12gb": "4cpu-12gb"
};

/**
 * Parse the authenticated public runtime-capability projection. This parser is
 * deliberately strict: CI must stop when the projection is absent or malformed
 * rather than silently shrinking the runtime matrix.
 */
export function parseRuntimeCapabilities(value) {
  const fail = (message) => {
    throw new Error(`invalid runtimeCapabilities: ${message}`);
  };
  if (!isRecord(value)) fail("expected an object");
  if (value.schemaVersion !== 2) fail("schemaVersion must be 2");
  if (!isNonEmptyString(value.capabilityVersion)) fail("capabilityVersion must be a non-empty string");
  if (typeof value.capabilityHash !== "string" || !CANONICAL_SHA256_DIGEST_PATTERN.test(value.capabilityHash)) {
    fail("capabilityHash must be a lowercase sha256 digest");
  }

  const availableRuntimeKinds = parseUniqueKnownStrings(
    value.availableRuntimeKinds,
    "availableRuntimeKinds",
    RUNTIME_KINDS,
    fail
  );
  if (availableRuntimeKinds.length === 0) fail("availableRuntimeKinds must not be empty");

  if (!isRecord(value.sizesByRuntimeKind)) fail("sizesByRuntimeKind must be an object");
  const sizesByRuntimeKind = {};
  for (const [runtime, sizes] of Object.entries(value.sizesByRuntimeKind)) {
    if (!RUNTIME_KINDS.includes(runtime)) fail(`sizesByRuntimeKind contains unknown runtime ${runtime}`);
    sizesByRuntimeKind[runtime] = parseUniqueKnownStrings(
      Array.isArray(sizes) ? sizes.map(canonicalRuntimeSize) : sizes,
      `sizesByRuntimeKind.${runtime}`,
      RUNTIME_SIZES,
      fail
    );
  }
  for (const runtime of availableRuntimeKinds) {
    if (!Object.hasOwn(sizesByRuntimeKind, runtime) || sizesByRuntimeKind[runtime].length === 0) {
      fail(`sizesByRuntimeKind.${runtime} must contain at least one size for an available runtime`);
    }
  }

  if (!isRecord(value.unavailable)) fail("unavailable must be an object");
  const unavailable = {};
  for (const [runtime, detail] of Object.entries(value.unavailable)) {
    if (!RUNTIME_KINDS.includes(runtime)) fail(`unavailable contains unknown runtime ${runtime}`);
    if (availableRuntimeKinds.includes(runtime)) fail(`${runtime} cannot be both available and unavailable`);
    if (!isRecord(detail) || !isNonEmptyString(detail.code)) {
      fail(`unavailable.${runtime}.code must be a non-empty string`);
    }
    unavailable[runtime] = { code: detail.code };
  }

  // TOTAL over runtime kinds, including ones this workspace may not name: a projection
  // that says which runtimes exist but not what they do is the gap the retired public
  // parity claim papered over, and CI must not accept it.
  if (!isRecord(value.profilesByRuntimeKind)) fail("profilesByRuntimeKind must be an object");
  const profilesByRuntimeKind = {};
  for (const runtime of RUNTIME_KINDS) {
    const profile = value.profilesByRuntimeKind[runtime];
    if (!isRecord(profile)) fail(`profilesByRuntimeKind.${runtime} must be an object`);
    if (profile.runtimeKind !== runtime) fail(`profilesByRuntimeKind.${runtime}.runtimeKind must be ${runtime}`);
    if (!isRecord(profile.capabilities)) fail(`profilesByRuntimeKind.${runtime}.capabilities must be an object`);
    if (!isRecord(profile.limits)) fail(`profilesByRuntimeKind.${runtime}.limits must be an object`);
    if (!isRecord(profile.delivery)) fail(`profilesByRuntimeKind.${runtime}.delivery must be an object`);
    profilesByRuntimeKind[runtime] = Object.freeze(profile);
  }
  for (const runtime of Object.keys(value.profilesByRuntimeKind)) {
    if (!RUNTIME_KINDS.includes(runtime)) fail(`profilesByRuntimeKind contains unknown runtime ${runtime}`);
  }

  return Object.freeze({
    schemaVersion: 2,
    capabilityVersion: value.capabilityVersion,
    capabilityHash: value.capabilityHash,
    availableRuntimeKinds: Object.freeze([...availableRuntimeKinds]),
    sizesByRuntimeKind: Object.freeze(sizesByRuntimeKind),
    unavailable: Object.freeze(unavailable),
    profilesByRuntimeKind: Object.freeze(profilesByRuntimeKind)
  });
}

function canonicalRuntimeSize(value) {
  return typeof value === "string" ? LEGACY_RUNTIME_SIZE_ALIASES[value] ?? value : value;
}

function parseUniqueKnownStrings(value, path, known, fail) {
  if (!Array.isArray(value)) fail(`${path} must be an array`);
  const result = [];
  for (const item of value) {
    if (typeof item !== "string" || !known.includes(item)) fail(`${path} contains unknown value ${String(item)}`);
    if (result.includes(item)) fail(`${path} contains duplicate value ${item}`);
    result.push(item);
  }
  return result;
}

function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function isNonEmptyString(value) {
  return typeof value === "string" && value.trim().length > 0;
}
