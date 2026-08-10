"use client";

import { useState } from "react";

import { submit } from "../client";
import { Notice } from "../components";
import type { PanelState } from "../panel";

/**
 * The human end of the CLI device flow.
 *
 * `aex auth login` prints a code and a URL. This is that URL. Until it existed
 * the CLI could poll forever: the approval half of the device state machine was
 * complete and had no caller anywhere, so `device_token_create` could only ever
 * answer `authorization_pending`.
 *
 * Two deliberate choices about the form:
 *
 * - **Approve is never the default action.** The code is prefilled from the
 *   query string for typing convenience, but the decision is always an explicit
 *   click. A page that approved on load would let any link approve a login.
 * - **The code is shown back, uppercased and hyphenated.** A person who typed
 *   the wrong code must be able to see that before they grant anything, and the
 *   platform's alphabet has no vowels and no digit-lookalikes precisely so this
 *   comparison is possible.
 */

/** The alphabet the platform mints user codes over: no vowels, no lookalikes. */
const USER_CODE = /^[BCDFGHJKLMNPQRSTVWXZ]{5}-[BCDFGHJKLMNPQRSTVWXZ]{5}$/;

interface DecisionResult {
  readonly decision: "approve" | "deny";
  readonly decidedAt: string;
  readonly scopes: readonly string[];
}

/** Normalizes what a person typed into what the route accepts. */
export function normalizeUserCode(raw: string): string | null {
  const stripped = raw.toLocaleUpperCase("en-US").replace(/[\s-]/g, "");
  if (stripped.length !== 10) return null;
  const hyphenated = `${stripped.slice(0, 5)}-${stripped.slice(5)}`;
  return USER_CODE.test(hyphenated) ? hyphenated : null;
}

export function DevicePanel({ prefill }: { prefill?: string | undefined }) {
  const [typed, setTyped] = useState(prefill ?? "");
  const [state, setState] = useState<PanelState<DecisionResult> | null>(null);
  const [pending, setPending] = useState(false);

  const normalized = normalizeUserCode(typed);
  const decided = state?.kind === "ready" ? state.data : null;

  async function decide(decision: "approve" | "deny") {
    if (normalized === null || pending) return;
    setPending(true);
    setState(
      await submit<DecisionResult>("device_decision_create", {
        body: { userCode: normalized, decision },
      }),
    );
    setPending(false);
  }

  if (decided) {
    return (
      <Notice
        status={decided.decision === "approve" ? "good" : "warning"}
        title={decided.decision === "approve" ? "Device approved" : "Device refused"}
      >
        <p className="small">
          {decided.decision === "approve"
            ? "Return to your terminal. The CLI will pick up its credential on its next poll."
            : "The code can never be redeemed. If this was not you, nothing further is needed."}
        </p>
      </Notice>
    );
  }

  return (
    <div className="stack">
      <div className="card">
        <div className="card-body stack">
          <label className="stack-tight" htmlFor="user-code">
            <span className="small">Code shown in your terminal</span>
            <input
              id="user-code"
              className="input mono"
              value={typed}
              autoComplete="off"
              spellCheck={false}
              placeholder="XXXXX-XXXXX"
              onChange={(event) => {
                setTyped(event.target.value);
                setState(null);
              }}
            />
          </label>
          {typed.length > 0 && normalized === null ? (
            <p className="small muted">
              That is not a code this platform mints. Ten letters, no vowels and no digits.
            </p>
          ) : null}
          <div className="stack-tight">
            <button
              type="button"
              className="button"
              disabled={normalized === null || pending}
              onClick={() => void decide("approve")}
            >
              Approve {normalized ?? ""}
            </button>
            <button
              type="button"
              className="button"
              disabled={normalized === null || pending}
              onClick={() => void decide("deny")}
            >
              This was not me
            </button>
          </div>
        </div>
      </div>

      {state && state.kind !== "ready" ? (
        <Notice status="serious" title="That code was not approved">
          <p className="small">
            {state.kind === "unavailable" || state.kind === "denied"
              ? state.failure.message
              : "The request did not complete. Try again."}
          </p>
        </Notice>
      ) : null}

      <p className="small muted">
        Approving grants a credential to whatever is running in that terminal, under your account.
        Only approve a code you are looking at right now.
      </p>
    </div>
  );
}
