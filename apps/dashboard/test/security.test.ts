import { expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { resolve } from "node:path";

import { authorizeDashboardRoute } from "../src/server/routes.js";
import { verifyCsrf } from "../src/server/csrf.js";
import { dashboardSessionCookie } from "../src/server/session.js";

test("off-allowlist routes are refused before credential attachment", () => {
  expect(authorizeDashboardRoute("session_get")).toEqual(expect.objectContaining({ id: "session_get" }));
  expect(authorizeDashboardRoute("session_create")).toBeNull();
});

test("CSRF requires matching tokens and rejects cross-site requests", () => {
  expect(verifyCsrf({ cookie: "token", header: "token", fetchSite: "same-origin" })).toBe(true);
  expect(verifyCsrf({ cookie: "token", header: "other", fetchSite: "same-origin" })).toBe(false);
  expect(verifyCsrf({ cookie: "token", header: "token", fetchSite: "cross-site" })).toBe(false);
});

test("dashboard session cookie has the host-only security envelope", () => {
  const cookie = dashboardSessionCookie("aex_ds_fixture", 600);
  expect(cookie).toContain("__Host-aex_session=");
  expect(cookie).toContain("HttpOnly");
  expect(cookie).toContain("Secure");
  expect(cookie).toContain("SameSite=Lax");
  expect(cookie).toContain("Path=/");
});

test("the source and dependency graph contain no durable-authority adapter", () => {
  const manifest = readFileSync(resolve(import.meta.dir, "../package.json"), "utf8");
  for (const forbidden of ["@aws-sdk/", "drizzle-orm", "kysely", "nodemailer", '"pg"']) {
    expect(manifest).not.toContain(forbidden);
  }
  const source = walk(resolve(import.meta.dir, "../src"))
    .map((path) => readFileSync(path, "utf8").toLowerCase())
    .join("\n");
  for (const forbidden of ["select ", "insert into", "delete from", "platformdb", "queryable"])
    expect(source).not.toContain(forbidden);
});

function walk(directory: string): string[] {
  return readdirSync(directory).flatMap((entry) => {
    const path = resolve(directory, entry);
    return statSync(path).isDirectory() ? walk(path) : [path];
  });
}
