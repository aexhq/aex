import { describe, expect, it } from "vitest";

import {
  requireSucceededRunBeforeFiles,
  finishedRunReadinessSource,
} from "../_fixtures/finished-run-readiness.js";

const failedResult = {
  sessionId: "ses_92a322388074120e884a27772f4dc313",
  status: "failed",
  ok: false,
  error: "run worker crashed before the brain loop started: mcp http_error (deepwiki): http 503",
  run: { runId: "ses_92a322388074120e884a27772f4dc313:turn:1" },
  events: [{
    type: "RUN_ERROR",
    data: {
      failureClass: "boot_failed",
      failureMessage: "mcp http_error with AEXSECRET-HEADER-VALUE",
    },
  }],
};

describe("finished run readiness before checkpoint-backed reads", () => {
  it("allows only the consistent succeeded result", () => {
    expect(() => requireSucceededRunBeforeFiles("probe", { status: "succeeded", ok: true })).not.toThrow();
  });

  it("surfaces the RUN_ERROR identity and cause instead of masking it with a files 409", () => {
    expect(() => requireSucceededRunBeforeFiles("probe", failedResult, ["AEXSECRET-HEADER-VALUE"])).toThrowError(
      /probe: run ended before checkpoint-backed file reads: .*ses_92a322388074120e884a27772f4dc313.*boot_failed.*mcp http_error.*\[REDACTED\]/,
    );
  });

  it("injects the same guard into clean-install child scripts", () => {
    const source = `${finishedRunReadinessSource()}\nreturn requireSucceededRunBeforeFiles;`;
    const childGuard = new Function(source)() as typeof requireSucceededRunBeforeFiles;
    expect(() => childGuard("child", failedResult, ["AEXSECRET-HEADER-VALUE"]))
      .toThrowError(/child: run ended before checkpoint-backed file reads/);
  });
});
