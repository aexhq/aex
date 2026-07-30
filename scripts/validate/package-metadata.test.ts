/**
 * The publishable-package metadata gate.
 *
 * WHY. Under the module release model every public package is independently
 * addressable on npm and publishes its own canary, so every public package —
 * not just `@aexhq/sdk` — needs a complete, correct manifest and its own copy of
 * the licence. Today that is untrue in ways nothing catches: `@aexhq/cli` is
 * `private: false` with no `license`, `bun pm pack` on any package produces a
 * tarball containing no licence text at all, and `packages/cli/dist/cli.mjs`
 * redistributes MIT-licensed `zod` and `fflate` with esbuild's
 * `legalComments: "none"` stripping their notices.
 *
 * The extraction adds 25 more packages in a single commit. A checklist would be
 * read once; this file is read on every push.
 *
 * Scope note: the registry is DERIVED (`private !== true`), so a package added
 * later is covered without editing this file. What is hard-coded here is only
 * what a derived rule cannot express — the known `aex` bin collision, and the
 * deliberate-violation fixture that proves the gate can fail.
 */
import { existsSync, readFileSync } from "node:fs";
import { relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "bun:test";
import {
  allPackageManifests,
  listPublishableModules,
  listWorkspaceModules,
  thirdPartyRuntimeDependencies,
  type PackageManifest
} from "../cicd/public-modules.js";
import { LEGAL_FILES, findDrift } from "../cicd/sync-package-legal.js";

const repoRoot = fileURLToPath(new URL("../..", import.meta.url));

/** The publish-facing fields, on top of what the registry module already reads. */
interface Manifest extends PackageManifest {
  readonly description?: string;
  readonly license?: string;
  readonly keywords?: readonly string[];
  readonly homepage?: string;
  readonly bugs?: { readonly url?: string };
  readonly repository?: { readonly type?: string; readonly url?: string; readonly directory?: string };
  readonly publishConfig?: { readonly access?: string };
  readonly files?: readonly string[];
  readonly bin?: Readonly<Record<string, string>>;
}

const SPDX = "Apache-2.0";
const ISSUES_URL = "https://github.com/aexhq/aex/issues";
const REPOSITORY_URL = "git+https://github.com/aexhq/aex.git";

/**
 * The publishable-metadata contract, as a pure function so the fixture below
 * can prove it rejects. Every rule here answers a question npm, a registry
 * page, or a licence obligation actually asks of a published tarball.
 */
export function metadataProblems(manifest: Manifest, relativeDir: string): string[] {
  const problems: string[] = [];
  const require_ = (condition: boolean, message: string): void => {
    if (!condition) problems.push(message);
  };

  require_(typeof manifest.name === "string" && manifest.name.startsWith("@aexhq/"), "name is not @aexhq-scoped");
  require_(typeof manifest.version === "string" && manifest.version.length > 0, "version is missing");
  require_(
    typeof manifest.description === "string" && manifest.description.trim().length >= 20,
    "description is missing or too short to be useful on a registry page"
  );
  require_(manifest.license === SPDX, `license is not "${SPDX}"`);
  require_(Array.isArray(manifest.keywords) && manifest.keywords.length > 0, "keywords are missing");
  require_(typeof manifest.homepage === "string" && manifest.homepage.length > 0, "homepage is missing");
  require_(manifest.bugs?.url === ISSUES_URL, `bugs.url is not ${ISSUES_URL}`);
  require_(manifest.repository?.url === REPOSITORY_URL, `repository.url is not ${REPOSITORY_URL}`);
  // Without `directory`, npm cannot link a monorepo package to its own source
  // and provenance tooling attributes every package to the repository root.
  require_(
    manifest.repository?.directory === relativeDir,
    `repository.directory is "${manifest.repository?.directory ?? "<absent>"}", expected "${relativeDir}"`
  );
  require_(manifest.publishConfig?.access === "public", "publishConfig.access is not \"public\"");
  require_(Array.isArray(manifest.files) && manifest.files.length > 0, "files is missing");
  for (const required of ["README.md", ...LEGAL_FILES]) {
    require_((manifest.files ?? []).includes(required), `files does not include ${required}`);
  }
  return problems;
}

const publishable = listPublishableModules(repoRoot);

describe("publishable package metadata", () => {
  it("resolves a non-empty public module registry", () => {
    // A derived registry that silently resolves to nothing would make every
    // assertion below vacuously true.
    expect(publishable.map((module) => module.manifest.name).sort()).toEqual([
      "@aexhq/cli",
      "@aexhq/contracts",
      "@aexhq/sdk"
    ]);
  });

  it("holds every publishable package to the full metadata contract", () => {
    const failures = publishable
      .flatMap((module) =>
        metadataProblems(module.manifest as Manifest, module.relativeDir).map(
          (problem) => `${module.relativeDir}: ${problem}`
        )
      )
      .sort();

    expect(failures).toEqual([]);
  });

  it("ships a README with every publishable package", () => {
    const missing = publishable
      .filter((module) => !existsSync(resolve(module.dir, "README.md")))
      .map((module) => `${module.relativeDir}/README.md`)
      .sort();

    expect(missing).toEqual([]);
  });

  it("rejects a manifest that omits any required field", () => {
    // The interlock. A gate that cannot fail is the same as no gate.
    const problems = metadataProblems({ name: "sdk", version: "1.0.0" }, "packages/sdk");

    expect(problems).toContain("name is not @aexhq-scoped");
    expect(problems).toContain(`license is not "${SPDX}"`);
    expect(problems).toContain("keywords are missing");
    expect(problems).toContain("files is missing");
    expect(problems).toContain("files does not include NOTICE");
  });
});

describe("licence declaration", () => {
  it("declares Apache-2.0 in every package.json in the repository", () => {
    // Not just the publishable ones: D1 is "Apache 2.0 throughout", and a
    // private workspace manifest with no licence is what the extraction would
    // copy 25 times.
    const undeclared = allPackageManifests(repoRoot)
      .filter((path) => (JSON.parse(readFileSync(path, "utf8")) as Manifest).license !== SPDX)
      .map((path) => relative(repoRoot, path).replaceAll("\\", "/"))
      .sort();

    expect(undeclared).toEqual([]);
  });

  it("carries the root LICENSE and NOTICE inside every publishable package", () => {
    // Apache 2.0 §4(a) obliges a redistributor to give recipients the licence,
    // and a tarball cannot reach up to a repository root outside it.
    expect(findDrift(repoRoot)).toEqual([]);
  });

  it("keeps the workspace root carrying both files", () => {
    for (const name of LEGAL_FILES) {
      expect(existsSync(resolve(repoRoot, name))).toBe(true);
    }
  });
});

describe("third-party attribution", () => {
  it("records every redistributed production dependency in NOTICE", () => {
    // `packages/cli/scripts/finalize-bundle.mjs` bundles the CLI's production
    // closure with `legalComments: "none"`, and `@aexhq/sdk` republishes that
    // bundle as its `aex` bin. The upstream MIT headers do not survive, so
    // NOTICE is the only place the attribution can live.
    const notice = readFileSync(resolve(repoRoot, "NOTICE"), "utf8");
    const dependencies = [
      ...new Set(
        publishable.flatMap((module) => thirdPartyRuntimeDependencies(module.manifest as Manifest))
      )
    ].sort();

    expect(dependencies.length).toBeGreaterThan(0);
    const unattributed = dependencies.filter((name) => !notice.includes(name));
    expect(unattributed).toEqual([]);
  });
});

describe("binary name ownership", () => {
  const claimants = listWorkspaceModules(repoRoot).filter((module) =>
    Object.keys((module.manifest as Manifest).bin ?? {}).includes("aex")
  );

  it("gives the standalone CLI sole ownership of the `aex` command", () => {
    expect(claimants.map((module) => module.manifest.name)).toEqual(["@aexhq/cli"]);
  });

  it("keeps the claimant publishable", () => {
    const publishableNames = new Set(publishable.map((module) => module.manifest.name));
    for (const module of claimants) {
      expect(publishableNames.has(module.manifest.name), `${module.manifest.name} claims \`aex\``).toBe(true);
    }
  });

  it("points the command at the standalone bundle", () => {
    const paths = new Set(claimants.map((module) => (module.manifest as Manifest).bin?.aex));
    expect([...paths]).toEqual(["./dist/cli.mjs"]);
  });

  it("documents the SDK as a separate library package", () => {
    const cliReadme = readFileSync(resolve(repoRoot, "packages/cli/README.md"), "utf8");
    const sdkReadme = readFileSync(resolve(repoRoot, "packages/sdk/README.md"), "utf8");
    expect(cliReadme).toContain("@aexhq/sdk");
    expect(sdkReadme).toContain("published separately by `@aexhq/cli`");
  });
});
