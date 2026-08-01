import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { ERROR_METADATA, ROUTES } from "../../src/index.js";

const workspaceRoot = resolve(import.meta.dir, "../../../..");

interface RegistryRoute {
  readonly operationId: string;
  readonly method: string;
  readonly path: string;
  readonly plane: string;
  readonly safeRetry: boolean;
  readonly idempotency: string;
  readonly transport: string;
  readonly pathParams: readonly string[];
  readonly queryParams: readonly string[];
  readonly pauseExempt: boolean;
}

interface RegistryError {
  readonly code: string;
  readonly class: string;
}

function registryRoutes(): Map<string, RegistryRoute> {
  const parsed = JSON.parse(
    readFileSync(resolve(workspaceRoot, "api/generated/registries/routes.json"), "utf8"),
  ) as { routes: RegistryRoute[] };
  return new Map(parsed.routes.map((route) => [route.operationId, route]));
}

function registryErrors(): Map<string, RegistryError> {
  const parsed = JSON.parse(
    readFileSync(resolve(workspaceRoot, "api/generated/registries/errors.json"), "utf8"),
  ) as { errors: RegistryError[] };
  return new Map(parsed.errors.map((row) => [row.code, row]));
}

test("every temporary route descriptor is field-identical to the contract registry", () => {
  const registry = registryRoutes();
  const ids = Object.keys(ROUTES);
  expect(ids.length).toBeGreaterThan(0);

  for (const id of ids) {
    const descriptor = ROUTES[id as keyof typeof ROUTES];
    const row = registry.get(id);
    expect(row).toBeDefined();
    const declared: Record<string, unknown> = {
      id: descriptor.id,
      method: descriptor.method,
      path: descriptor.path,
      plane: descriptor.plane,
      safeRetry: descriptor.safeRetry,
      idempotency: descriptor.idempotency,
      transport: descriptor.transport,
      pathParams: [...descriptor.pathParams],
      queryParams: [...descriptor.queryParams],
      pauseExempt: descriptor.pauseExempt,
    };
    expect(declared).toEqual({
      id: row?.operationId,
      method: row?.method,
      path: row?.path,
      plane: row?.plane,
      safeRetry: row?.safeRetry,
      idempotency: row?.idempotency,
      transport: row?.transport,
      pathParams: [...(row?.pathParams ?? [])],
      queryParams: [...(row?.queryParams ?? [])],
      pauseExempt: row?.pauseExempt,
    });
  }
});

test("every declared path placeholder appears in the declared path", () => {
  for (const descriptor of Object.values(ROUTES)) {
    for (const parameter of descriptor.pathParams) {
      expect(descriptor.path).toContain(`{${parameter}}`);
    }
    const placeholders = [...descriptor.path.matchAll(/\{([a-zA-Z]+)\}/g)].map((match) => match[1]);
    expect(placeholders).toEqual([...descriptor.pathParams]);
  }
});

test("the error class table is the whole contract error registry", () => {
  const registry = registryErrors();
  expect(Object.keys(ERROR_METADATA).sort()).toEqual([...registry.keys()].sort());
  for (const [code, row] of registry) {
    expect(ERROR_METADATA[code]?.class as string | undefined).toBe(row.class);
  }
});
