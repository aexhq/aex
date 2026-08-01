import { expect, test } from "bun:test";
import { USER_SCENARIOS } from "../../scenarios.js";
test("operator scenarios remain visible in public source", () => { expect(USER_SCENARIOS.filter(({ suite }) => suite === "operator")).toHaveLength(2); });
