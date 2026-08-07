import { expect, test } from "bun:test";
import { USER_SCENARIOS } from "../../scenarios.js";

test("all packed scenarios declare an external-consumer artifact", () => {
  const scenarios = USER_SCENARIOS.filter(({ suite }) => suite === "packed");
  expect(scenarios).toHaveLength(19);
  for (const scenario of scenarios) expect(scenario.artifacts.some((artifact) => artifact === "sdk" || artifact === "cli")).toBe(true);
});
