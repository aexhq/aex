import { expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { resolve } from "node:path";
import { ROUTES, regionalHost, type RouteId } from "@aexhq/sdk";

import { DASHBOARD_ROUTES } from "../src/server/routes";
import { REGIONS } from "../src/ui/regions";

const root = resolve(import.meta.dir, "..");
const workspaceRoot = resolve(root, "../..");

test("every allowlist entry is a real operation and the list has no duplicates", () => {
  expect(new Set(DASHBOARD_ROUTES).size).toBe(DASHBOARD_ROUTES.length);
  for (const routeId of DASHBOARD_ROUTES) {
    expect(ROUTES[routeId]).toEqual(expect.objectContaining({ id: routeId }));
  }
});

test("the allowlist is exactly the essential personal dashboard surface", () => {
  for (const routeId of DASHBOARD_ROUTES as readonly string[]) {
    expect(routeId.startsWith("admin_")).toBe(false);
    expect(routeId.startsWith("operator_")).toBe(false);
    expect(routeId.startsWith("otlp_")).toBe(false);
  }
  expect(DASHBOARD_ROUTES).toEqual([
    "api_keys_list",
    "api_key_create",
    "api_key_revoke",
    "billing_balance_get",
    "billing_payment_method_delete",
    "billing_payment_method_session_create",
    "billing_payment_methods_list",
    "billing_top_up_checkout_create",
    "billing_transactions_list",
    "billing_usage_get",
    "registry_files_delete",
    "registry_files_download_create",
    "registry_files_get",
    "registry_files_list",
    "registry_files_put",
    "upload_create",
    "upload_complete",
    "sessions_list",
    "session_create",
    "session_get",
    "session_cancel",
    "session_delete",
    "session_message_send",
    "session_messages_list",
    "session_messages_stream",
    "session_telemetry_download_create",
    "session_telemetry_replay",
    "session_telemetry_stream",
    "session_terminate",
  ]);
});

test("the bootstrap operation is not reachable through the generic passthrough", () => {
  for (const dedicated of [
    "dashboard_bootstrap_get",
    "dashboard_session_create",
    "dashboard_session_delete",
  ]) {
    expect((DASHBOARD_ROUTES as readonly string[]).includes(dedicated)).toBe(false);
  }
  expect(ROUTES.dashboard_bootstrap_get.plane).toBe("central");
});

test("every operation the UI names is on the allowlist", () => {
  const identifiers = new Set(Object.keys(ROUTES));
  const referenced = new Set<string>();
  for (const file of walk(resolve(root, "src/ui")).filter((path) => /\.(ts|tsx)$/.test(path))) {
    const source = readFileSync(file, "utf8");
    for (const match of source.matchAll(/"([a-z][a-z0-9_]+)"/g)) {
      const candidate = match[1];
      if (candidate && identifiers.has(candidate)) referenced.add(candidate);
    }
  }
  expect(referenced.size).toBeGreaterThan(0);
  for (const routeId of referenced) {
    expect({ routeId, allowed: (DASHBOARD_ROUTES as readonly string[]).includes(routeId) })
      .toEqual({ routeId, allowed: true });
  }
});

test("the region table agrees with the SDK's own regional hosts", () => {
  const registry = JSON.parse(
    readFileSync(resolve(workspaceRoot, "api/generated/schemas/Workspace.json"), "utf8"),
  ) as { properties: { region: { enum: string[] } } };
  const declared: string[] = REGIONS.map((row) => row.region);
  expect(declared.sort()).toEqual([...registry.properties.region.enum].sort());
  for (const row of REGIONS) {
    expect(regionalHost(row.code)).toBe(`https://${row.region}.api.aex.dev`);
  }
});

test("every allowlisted regional operation can be addressed by a known region", () => {
  const regional = (DASHBOARD_ROUTES as readonly RouteId[]).filter((id) => ROUTES[id].plane === "regional");
  expect(regional.length).toBeGreaterThan(0);
  for (const row of REGIONS) {
    expect(regionalHost(row.code).startsWith("https://")).toBe(true);
  }
});

function walk(directory: string): string[] {
  return readdirSync(directory).flatMap((entry) => {
    const path = resolve(directory, entry);
    return statSync(path).isDirectory() ? walk(path) : [path];
  });
}
