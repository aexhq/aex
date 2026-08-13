import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { DEADLINE_MS, panelUrl, requestKey } from "../src/ui/client";

describe("panel request identity", () => {
  test("two structurally equal option objects produce the same key", () => {
    const first = requestKey("sessions_list", {
      region: "euw1",
      parameters: { limit: "50", status: "running" },
      deadlineMs: DEADLINE_MS.control,
    }, 0);
    const second = requestKey("sessions_list", {
      region: "euw1",
      parameters: { status: "running", limit: "50" },
      deadlineMs: DEADLINE_MS.control,
    }, 0);
    expect(first).toBe(second);
  });

  test("an undefined parameter is the same request as an absent one", () => {
    expect(requestKey("sessions_list", { region: "euw1", parameters: { limit: "50", status: undefined } }, 0))
      .toBe(requestKey("sessions_list", { region: "euw1", parameters: { limit: "50" } }, 0));
  });

  test("anything that changes the request changes the key", () => {
    const base = { region: "euw1", parameters: { limit: "50" } } as const;
    const key = requestKey("sessions_list", base, 0);
    expect(requestKey("sessions_list", { ...base, region: "use1" }, 0)).not.toBe(key);
    expect(requestKey("sessions_list", { ...base, parameters: { limit: "10" } }, 0)).not.toBe(key);
    expect(requestKey("sessions_list", { ...base, deadlineMs: DEADLINE_MS.analytics }, 0)).not.toBe(key);
    expect(requestKey("sessions_list", { ...base, enabled: false }, 0)).not.toBe(key);
    expect(requestKey("session_get", base, 0)).not.toBe(key);
    expect(requestKey("sessions_list", base, 1)).not.toBe(key);
  });

});

test("the request effect depends on the value key alone, never on an object literal", () => {
  const source = readFileSync(resolve(import.meta.dir, "../src/ui/client.ts"), "utf8");
  const effects = [...source.matchAll(/useEffect\(/g)].length;
  const dependencies = [...source.matchAll(/\}, \[([^\]]*)\]\);/g)].map((match) => match[1]!.trim());
  expect({ effects, dependencies }).toEqual({ effects: 1, dependencies: ["key, routeId"] });
});

describe("panel URLs", () => {
  test("a regional operation carries its region and a central one does not", () => {
    expect(panelUrl("sessions_list", { limit: "50" }, "euw1"))
      .toBe("/api/v1/regional/sessions_list?region=euw1&limit=50");
    expect(panelUrl("api_keys_list", { workspaceId: "wsp_1" }))
      .toBe("/api/v1/central/api_keys_list?workspaceId=wsp_1");
  });

  test("a regional operation with no region is a programming error, not a silent default", () => {
    expect(() => panelUrl("sessions_list", {})).toThrow("region");
  });

  test("an undefined parameter is omitted rather than sent as the string undefined", () => {
    expect(panelUrl("sessions_list", { limit: "50", status: undefined }, "euw1"))
      .toBe("/api/v1/regional/sessions_list?region=euw1&limit=50");
  });
});
