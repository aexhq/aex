import { readdirSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "bun:test";

import { readModuleGraph } from "../cicd/public-module-graph.mjs";
import {
  jobNeeds,
  readRepoFile,
  readWorkflow,
  workflowJob,
  workflowSteps,
  workflowTriggers
} from "./workflow-test-helpers.js";

const repoRoot = new URL("../../", import.meta.url);
const ci = readWorkflow(".github/workflows/ci.yml");
const promote = readWorkflow(".github/workflows/promote-canary.yml");
const workflowPaths = readdirSync(new URL(".github/workflows/", repoRoot))
  .filter((name) => name.endsWith(".yml") || name.endsWith(".yaml"))
  .map((name) => `.github/workflows/${name}`)
  .sort();

function stepRun(job: ReturnType<typeof workflowJob>, matcher: RegExp): boolean {
  return workflowSteps(job).some((step) => typeof step.run === "string" && matcher.test(step.run));
}

describe("the module canary loop runs the dependent closure before it publishes", () => {
  it("routes once, in a dedicated job every module lane reads", () => {
    const modules = workflowJob(ci, "modules");
    expect(stepRun(modules, /public-module-graph\.mjs/)).toBe(true);
    for (const output of ["routing_failed", "closure", "publish", "checks_matrix", "publish_matrix", "has_publish"]) {
      expect(Object.keys(modules.outputs ?? {}), output).toContain(output);
    }
  });

  it("runs module lanes over the dependent closure, not the changed module alone", () => {
    const checks = workflowJob(ci, "module-checks");
    expect(jobNeeds(checks)).toContain("modules");
    expect(JSON.stringify(checks.strategy?.matrix)).toContain("checks_matrix");
    // fail-fast would cancel a sibling module's lanes, hiding a second failure.
    expect(checks.strategy?.["fail-fast"]).toBe(false);
    expect(stepRun(checks, /run-module-checks\.mjs/)).toBe(true);
  });

  it("makes the module lanes blocking rather than advisory", () => {
    const required = workflowJob(ci, "checks");
    expect(jobNeeds(required)).toContain("module-checks");
    // `skipped` is tolerated only because an empty closure means nothing changed;
    // every other non-success must fail the required check.
    const body = JSON.stringify(workflowSteps(required));
    expect(body).toContain("MODULE_CHECKS_RESULT");
    expect(body).toContain("!= skipped");
  });

  it("publishes one canary per affected module, driven by the router's publish set", () => {
    const publish = workflowJob(ci, "publish-canary");
    expect(JSON.stringify(publish.strategy?.matrix)).toContain("publish_matrix");
    expect(publish.strategy?.["fail-fast"]).toBe(false);
    expect(publish.strategy?.["max-parallel"]).toBeUndefined();
    for (const need of ["modules", "lint", "typecheck", "unit-tests", "module-checks"]) {
      expect(jobNeeds(publish), need).toContain(need);
    }
  });

  it("publishes only from a green main push, never from a pull request", () => {
    const publish = workflowJob(ci, "publish-canary");
    const guard = String(publish.if);
    expect(guard).toContain("github.event_name == 'push'");
    expect(guard).toContain("github.ref == 'refs/heads/main'");
    for (const lane of ["lint", "typecheck", "unit-tests", "module-checks"]) {
      expect(guard, lane).toContain(`needs.${lane}.result == 'success'`);
    }
  });

  it("withholds publication when routing failed, and widens verification instead", () => {
    // Verification fails OPEN, publication fails CLOSED. The router returns the
    // full closure with an empty publish set on its error path; the workflow's
    // guard is the other half of that contract.
    const publish = workflowJob(ci, "publish-canary");
    expect(String(publish.if)).toContain("needs.modules.outputs.routing_failed == 'false'");
    expect(String(publish.if)).toContain("needs.modules.outputs.has_publish == 'true'");
    expect(String(workflowJob(ci, "canary-manifest").if)).toContain(
      "needs.modules.outputs.routing_failed == 'false'"
    );
  });
});

describe("each published canary records version, integrity, and source identity", () => {
  const publish = workflowJob(ci, "publish-canary");

  it("resolves a per-module version and an immutable per-module source tag", () => {
    expect(stepRun(publish, /module-canary\.mjs resolve/)).toBe(true);
    expect(stepRun(publish, /module-canary\.mjs source-tag/)).toBe(true);
    expect(stepRun(publish, /git tag --annotate/)).toBe(true);
  });

  it("feeds the resolver both identity inputs, so two pushes cannot collide", () => {
    // The version is a function of (base, source sha, run id). Dropping either
    // suffix input puts every push at one base back on one version string, which
    // is the failure that reds the SECOND green push and nothing else catches
    // until npm refuses the publish.
    const resolve = workflowSteps(publish).find(
      (step) => typeof step.run === "string" && /module-canary\.mjs resolve/.test(step.run)
    );
    expect(String(resolve?.run)).toContain('--sha "$RELEASE_HEAD_SHA"');
    expect(String(resolve?.run)).toContain('--run "$RELEASE_RUN_ID"');
    expect(String((resolve?.env as Record<string, string> | undefined)?.RELEASE_RUN_ID)).toContain("github.run_id");
  });

  it("refuses to reuse a version npm already served", () => {
    expect(stepRun(publish, /assert-npm-version-available\.mjs/)).toBe(true);
  });

  it("takes the integrity value from the registry rather than computing its own", () => {
    expect(stepRun(publish, /wait-for-npm\.mjs/)).toBe(true);
    expect(JSON.stringify(workflowSteps(publish))).toContain("registry-evidence");
  });

  it("records the identity as an artifact the private release can select from", () => {
    expect(stepRun(publish, /canary-release-/)).toBe(true);
    const manifest = workflowJob(ci, "canary-manifest");
    expect(stepRun(manifest, /canary-manifest\.mjs build/)).toBe(true);
    expect(stepRun(manifest, /canary-manifest\.mjs check/)).toBe(true);
  });

  it("never packs around the public-surface boundary check", () => {
    // Calling `bun pm pack` directly once skipped contracts:boundary:check, so a
    // public-surface violation could ship with every gate green.
    const pack = workflowSteps(publish).find((step) => step.id === "pack");
    expect(String(pack?.run)).toMatch(/pack:sdk|contracts:boundary:check/);
    expect(String(pack?.run)).toContain("contracts:boundary:check");
  });

  it("waits for same-run upstreams before packing and verifies packed metadata", () => {
    const steps = workflowSteps(publish);
    const waitIndex = steps.findIndex((step) => String(step.run).includes("upstream-lines"));
    const packIndex = steps.findIndex((step) => step.id === "pack");
    const publishIndex = steps.findIndex((step) => String(step.run).includes("npm publish"));
    expect(waitIndex).toBeGreaterThanOrEqual(0);
    expect(packIndex).toBeGreaterThan(waitIndex);
    expect(publishIndex).toBeGreaterThan(packIndex);
    expect(String(steps[waitIndex]?.run)).toContain("wait-for-npm.mjs");
    expect(String(steps[packIndex]?.run)).toContain("verify-packed");
    expect(JSON.stringify(steps)).toContain("steps.pack.outputs.upstream");
  });
});

describe("publication is OIDC trusted publishing with no token path", () => {
  it("requests an id-token and publishes with provenance", () => {
    const publish = workflowJob(ci, "publish-canary");
    expect((publish.permissions as Record<string, string>)["id-token"]).toBe("write");
    expect(stepRun(publish, /npm publish[\s\S]*--provenance/)).toBe(true);
  });

  it("reintroduces no npm credential anywhere in the public workflows", () => {
    // Secret REFERENCES, not prose: the workflows explain why the token path was
    // removed, and a naive substring match would forbid saying so.
    const credential = /secrets\.[A-Za-z_]*(?:NPM|NODE_AUTH)[A-Za-z_]*|NODE_AUTH_TOKEN\s*:|_authToken\s*=/;
    for (const path of workflowPaths) {
      expect(readRepoFile(path), `${path} must not reference an npm token secret`).not.toMatch(credential);
    }
  });

  it("keeps every job on a GitHub-hosted runner behind a declared environment", () => {
    expect(workflowJob(ci, "publish-canary").environment).toBe("npm-release");
    expect(workflowJob(promote, "promote").environment).toBe("npm-promote");
  });
});

describe("stable promotion stays manual", () => {
  it("has exactly one trigger, and it is a human dispatch", () => {
    expect(Object.keys(workflowTriggers(promote))).toEqual(["workflow_dispatch"]);
  });

  it("is not reachable from a push, a release, a schedule, or another workflow", () => {
    const triggers = workflowTriggers(promote);
    for (const forbidden of ["push", "release", "schedule", "workflow_run", "workflow_call", "pull_request"]) {
      expect(Object.keys(triggers), forbidden).not.toContain(forbidden);
    }
  });

  it("verifies the selection against the registry before it moves a tag", () => {
    const job = workflowJob(promote, "promote");
    expect(stepRun(job, /verify-canary-selection\.mjs/)).toBe(true);
    expect(stepRun(job, /merge-base --is-ancestor/)).toBe(true);
    const steps = workflowSteps(job);
    const verifyIndex = steps.findIndex((step) => String(step.run).includes("verify-canary-selection.mjs"));
    const promoteIndex = steps.findIndex((step) => String(step.run).includes("npm dist-tag add"));
    expect(verifyIndex).toBeGreaterThanOrEqual(0);
    expect(promoteIndex).toBeGreaterThan(verifyIndex);
  });

  it("is never invoked from the CI workflow, so green CI cannot promote", () => {
    // Prose may name it — CI explains where promotion lives. What must not exist
    // is an invocation: a reusable-workflow call or a dist-tag move inside CI.
    for (const [id, job] of Object.entries(ci.jobs)) {
      expect(String(job.uses ?? ""), `${id} must not call the promotion workflow`).not.toContain("promote-canary");
      expect(JSON.stringify(workflowSteps(job)), id).not.toContain("npm dist-tag");
    }
    expect(Object.keys(workflowTriggers(ci))).not.toContain("workflow_run");
  });
});

describe("the publish set is exactly the workspace's publishable modules", () => {
  it("never offers a private workspace package to npm", () => {
    const graph = readModuleGraph(resolve(import.meta.dir, "..", ".."));
    const publishable = graph.modules.filter((node) => node.publishable).map((node) => node.name);
    expect(publishable.sort()).toEqual(["@aexhq/cli", "@aexhq/contracts", "@aexhq/sdk"]);
    for (const node of graph.modules) {
      if (node.publishable) continue;
      expect(node.name, `${node.name} is private and must never enter the publish matrix`).toMatch(
        /^@aexhq\/(docs|user-tests)$/
      );
    }
  });
});
