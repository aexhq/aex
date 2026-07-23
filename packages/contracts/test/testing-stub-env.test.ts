import { afterEach, describe, expect, it } from "bun:test";
import { restoreEnv, stubEnv } from "../src/testing.js";

const KEY = "AEX_TESTING_STUB_ENV_PROBE";
const OTHER = "AEX_TESTING_STUB_ENV_PROBE_OTHER";

afterEach(() => {
  restoreEnv();
  delete process.env[KEY];
  delete process.env[OTHER];
});

describe("stubEnv", () => {
  it("sets a variable and the returned restore fn reinstates the previous value", () => {
    process.env[KEY] = "before";
    const restore = stubEnv(KEY, "after");
    expect(process.env[KEY]).toBe("after");
    restore();
    expect(process.env[KEY]).toBe("before");
  });

  it("restores absence when the variable was not set", () => {
    delete process.env[KEY];
    const restore = stubEnv(KEY, "value");
    expect(process.env[KEY]).toBe("value");
    restore();
    expect(KEY in process.env).toBe(false);
  });

  it("deletes the variable when stubbed to undefined and restores it after", () => {
    process.env[KEY] = "present";
    const restore = stubEnv(KEY, undefined);
    expect(KEY in process.env).toBe(false);
    restore();
    expect(process.env[KEY]).toBe("present");
  });

  it("is idempotent: calling the restore fn twice does not clobber later state", () => {
    process.env[KEY] = "original";
    const restore = stubEnv(KEY, "stubbed");
    restore();
    process.env[KEY] = "moved-on";
    restore();
    expect(process.env[KEY]).toBe("moved-on");
  });

  it("restores stacked stubs on the same key in LIFO order via restoreEnv()", () => {
    process.env[KEY] = "original";
    stubEnv(KEY, "first");
    stubEnv(KEY, "second");
    expect(process.env[KEY]).toBe("second");
    restoreEnv();
    expect(process.env[KEY]).toBe("original");
  });

  it("restoreEnv() restores every outstanding stub across keys", () => {
    process.env[KEY] = "a";
    delete process.env[OTHER];
    stubEnv(KEY, "x");
    stubEnv(OTHER, "y");
    restoreEnv();
    expect(process.env[KEY]).toBe("a");
    expect(OTHER in process.env).toBe(false);
  });

  it("restoreEnv() skips stubs already restored individually", () => {
    process.env[KEY] = "base";
    const restoreFirst = stubEnv(KEY, "one");
    stubEnv(OTHER, "two");
    restoreFirst();
    process.env[KEY] = "user-set-after-restore";
    restoreEnv();
    expect(process.env[KEY]).toBe("user-set-after-restore");
    expect(OTHER in process.env).toBe(false);
  });

  it("rejects a non-string key loudly", () => {
    expect(() => stubEnv("", "v")).toThrow(TypeError);
  });
});
