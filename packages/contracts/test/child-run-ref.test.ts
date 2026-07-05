/**
 * WS8 class-killer: a child run handed to you is RESOLVABLE. ChildRunRef carries
 * the same resolvable-handle capability as a run (a real `id` the run facade
 * accepts), so a child can never be surfaced as a bare unresolvable string.
 */
import { describe, expect, it } from "vitest";
import type { ChildRunRef, ResolvableRunRef } from "../src/index.js";

describe("ChildRunRef resolvable-handle invariant (WS8)", () => {
  it("[compile-time] a ChildRunRef is a ResolvableRunRef (its id resolves through the run facade)", () => {
    const child: ChildRunRef = { id: "run_child_1", parentRunId: "run_parent", status: "succeeded" };
    const resolvable: ResolvableRunRef = child; // handed ⇒ resolvable
    expect(resolvable.id).toBe("run_child_1");
  });

  it("[compile-time] a bare string is NOT a resolvable run ref", () => {
    // @ts-expect-error — a child cannot be surfaced as a bare unresolvable string
    const bad: ResolvableRunRef = "run_child_1";
    void bad;
  });
});
