/**
 * WS8 class-killer: a child session handed to you is RESOLVABLE. ChildSessionRef carries
 * the same resolvable-handle capability as a session (a real `id` the session record facade
 * accepts), so a child can never be surfaced as a bare unresolvable string.
 */
import { describe, expect, it } from "vitest";
import type { ChildSessionRef, ResolvableSessionRef } from "../src/index.js";

describe("ChildSessionRef resolvable-handle invariant (WS8)", () => {
  it("[compile-time] a ChildSessionRef is a ResolvableSessionRef (its id resolves through the session record facade)", () => {
    const child: ChildSessionRef = { id: "ses_child_1", parentSessionId: "ses_parent", status: "succeeded" };
    const resolvable: ResolvableSessionRef = child; // handed ⇒ resolvable
    expect(resolvable.id).toBe("ses_child_1");
  });

  it("[compile-time] a bare string is NOT a resolvable run ref", () => {
    // @ts-expect-error — a child cannot be surfaced as a bare unresolvable string
    const bad: ResolvableSessionRef = "ses_child_1";
    void bad;
  });
});
