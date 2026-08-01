import { expect, test } from "bun:test";
import { USER_SCENARIOS } from "../../scenarios.js";
test("money scenarios are explicitly classified", () => { expect(USER_SCENARIOS.filter(({ suite }) => suite === "money")).toHaveLength(3); });
