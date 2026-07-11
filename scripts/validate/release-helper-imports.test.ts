import { describe, expect, it } from "vitest";

describe("release helper module imports", () => {
  it("loads the canary version helper through Vitest", async () => {
    // @ts-expect-error JavaScript release helper is validated directly.
    const helper = await import("../cicd/canary-version.mjs");

    expect(helper.buildCanaryVersion).toBeTypeOf("function");
    expect(helper.applySdkVersion).toBeTypeOf("function");
  });

  it("loads the monotonic promotion helper through Vitest", async () => {
    // @ts-expect-error JavaScript release helper is validated directly.
    const helper = await import("../cicd/assert-monotonic-promotion.mjs");

    expect(helper.assertMonotonicPromotion).toBeTypeOf("function");
  });
});
