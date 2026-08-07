import { expect, test } from "bun:test";
import { USER_SCENARIOS } from "../../scenarios.js";
test("live scenarios fail closed onto dev descriptors", () => { for (const row of USER_SCENARIOS.filter(({ suite }) => suite === "live")) expect(row.plane).toBe("dev"); });
