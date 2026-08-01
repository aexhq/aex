import { expect, test } from "bun:test";
import { USER_SCENARIOS } from "../../scenarios.js";
test("browser scenarios own the dashboard artifact", () => { for (const row of USER_SCENARIOS.filter(({ suite }) => suite === "browser")) expect(row.artifacts).toContain("dashboard"); });
