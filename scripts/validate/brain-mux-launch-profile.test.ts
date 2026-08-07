import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const root = resolve(import.meta.dir, "../..");
const read = (path: string): string => readFileSync(resolve(root, path), "utf8");

// The approved first-alpha profile is 16 active activations per task. It is stated
// twice by necessity — once in the binary that derives its admission bands from it,
// once in the Terraform module that pins what the task definition may carry — because
// neither language can read the other. Two literals with nothing between them is how
// `desired_count` came to be pinned in two places that silently disagreed, so this
// holds them to each other instead.
const APPROVED_ACTIVATION_BUDGET = 16;
const BUDGET_VAR = "AEX_MAX_ACTIVE_ACTIVATIONS";

const rustDefaultBand = (source: string, field: string): number => {
  const match = new RegExp(`\\b${field}:\\s*([0-9_]+)\\s*,`).exec(source);
  if (match?.[1] === undefined) {
    throw new Error(`AdmissionBounds::default has no \`${field}\` band`);
  }
  return Number(match[1].replaceAll("_", ""));
};

describe("brain-mux launch profile", () => {
  test("the deployment pin and the composed admission bands name one budget", () => {
    const admission = read("runtimes/brain-mux/src/admission.rs");
    const variables = read("infra/modules/ecs-service/variables.tf");

    // The bands the binary composes from the budget: target, 2x, 5x.
    expect(rustDefaultBand(admission, "target")).toBe(APPROVED_ACTIVATION_BUDGET);
    expect(rustDefaultBand(admission, "safety_cap")).toBe(APPROVED_ACTIVATION_BUDGET * 2);
    expect(rustDefaultBand(admission, "offered_ceiling")).toBe(APPROVED_ACTIVATION_BUDGET * 5);

    // The one value a `brain-mux` task definition may carry.
    expect(variables).toContain(
      `var.name != "brain-mux" || lookup(var.env, "${BUDGET_VAR}", "") == "${APPROVED_ACTIVATION_BUDGET}"`
    );
  });

  test("the budget the module pins is the variable the binary requires", () => {
    const main = read("runtimes/brain-mux/src/main.rs");
    expect(main).toContain(`pub const BUDGET_VAR: &str = "${BUDGET_VAR}";`);
    // Required, not defaulted: `required` refuses an absent or blank value, which is
    // why the module makes the variable mandatory rather than optional.
    expect(main).toContain("let raw_budget = required(&lookup, BUDGET_VAR)?;");
  });
});
