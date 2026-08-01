import { expect, test } from "bun:test";
import { USER_SCENARIOS } from "../../scenarios.js";
test("local scenarios need no remote plane", () => { for (const row of USER_SCENARIOS.filter(({ suite }) => suite === "local")) expect(row.plane).toBe("local"); });
