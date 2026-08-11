import { expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { resolve } from "node:path";

import { authorizeDashboardRoute } from "../src/server/routes";
import { verifyCsrf } from "../src/server/csrf";
import { dashboardSessionCookie } from "../src/server/session";

const root = resolve(import.meta.dir, "..");

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

/**
 * The hard rule, scanned rather than trusted.
 *
 * SQL is looked for where SQL can only live — inside a string or template literal —
 * and matched as a statement shape rather than as a substring. The previous version
 * matched a lowercase `"select "` anywhere, which also fires on `<select>` in a
 * form, and so read as a reason to avoid the accessible control rather than a reason
 * to avoid SQL. Statement coverage is unchanged and the scan now covers `app/**` as
 * well as `src/**`; the adapter identifiers below catch the imports that would
 * produce a statement in the first place.
 */
const SQL_SHAPES: readonly RegExp[] = [
  /\bselect\b[\s\S]{0,400}?\bfrom\b/i,
  /\binsert\s+into\b/i,
  /\bdelete\s+from\b/i,
  /\bupdate\b\s+[\w".]+\s+\bset\b/i,
  /\bcreate\s+table\b/i,
];

const ADAPTER_IDENTIFIERS: readonly string[] = [
  "PlatformDb",
  "Queryable",
  "getDashboardDb",
  "getDashboardRepository",
  "getPlatformRepository",
  "AuthorizationRepository",
  "createPgPool",
  "DataApiDb",
  "awsCredentialsProvider",
];

const FORBIDDEN_ENVIRONMENT: readonly string[] = [
  "DATABASE_URL",
  "AURORA_CLUSTER_ARN",
  "AURORA_SECRET_ARN",
  "AWS_ROLE_ARN",
  "AEX_API_KEY_PEPPER",
  "AEX_ACCOUNT_TOKEN_PEPPER",
  "AEX_SHARED_SECRET",
  "AEX_TEST_AUTH",
  "AEX_E2E",
  "OAUTH_GOOGLE_CLIENT_SECRET",
];

test("the manifest holds no durable-authority adapter", () => {
  const manifest = readFileSync(resolve(root, "package.json"), "utf8");
  for (const forbidden of [
    "@aws-sdk/",
    "@aexhq/db",
    "drizzle-orm",
    "kysely",
    "nodemailer",
    '"pg"',
    "@vercel/oidc",
  ]) {
    expect(manifest).not.toContain(forbidden);
  }
});

/** Every quoted string and template literal in a source file. */
function literals(source: string): readonly string[] {
  const found = source.match(/"(?:[^"\\\n]|\\.)*"|'(?:[^'\\\n]|\\.)*'|`(?:[^`\\]|\\.)*`/g);
  return found ?? [];
}

test("no source file under app or src issues SQL or names a repository adapter", () => {
  const files = sources();
  expect(files.length).toBeGreaterThan(0);
  for (const file of files) {
    const source = readFileSync(file, "utf8");
    for (const literal of literals(source)) {
      for (const shape of SQL_SHAPES) {
        expect({ file, literal, shape: shape.source, matched: shape.test(literal) })
          .toEqual({ file, literal, shape: shape.source, matched: false });
      }
    }
    for (const identifier of ADAPTER_IDENTIFIERS) {
      expect({ file, identifier, present: source.includes(identifier) })
        .toEqual({ file, identifier, present: false });
    }
  }
});

test("the SQL scan still catches a statement that a substring scan would have caught", () => {
  const planted = 'const query = "select id from workspaces where slug = $1";';
  expect(literals(planted).some((literal) => SQL_SHAPES.some((shape) => shape.test(literal)))).toBe(true);
  const innocent = '<select value={status}>{"loaded from the API"}</select>';
  expect(literals(innocent).some((literal) => SQL_SHAPES.some((shape) => shape.test(literal)))).toBe(false);
});

test("no source file reads a durable-authority environment variable", () => {
  for (const file of sources()) {
    const source = readFileSync(file, "utf8");
    for (const name of FORBIDDEN_ENVIRONMENT) {
      expect({ file, name, present: source.includes(name) }).toEqual({ file, name, present: false });
    }
  }
});

function sources(): string[] {
  return [...walk(resolve(root, "src")), ...walk(resolve(root, "app"))]
    .filter((path) => /\.(ts|tsx)$/.test(path));
}

function walk(directory: string): string[] {
  return readdirSync(directory).flatMap((entry) => {
    const path = resolve(directory, entry);
    return statSync(path).isDirectory() ? walk(path) : [path];
  });
}
