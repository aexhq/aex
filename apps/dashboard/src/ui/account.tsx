import type { AccountOperationalState } from "../server/bootstrap";
import { Badge, Notice } from "./components";
import { centsToUsd } from "./panel";

/**
 * The paused account is a product state, not an error.
 *
 * It appears exactly once, in the shell, with the remedy attached. No panel repeats
 * it; panels whose operations are not pause-exempt render the `paused` panel state
 * instead, which points back here.
 */
export function AccountBanner({
  account,
}: {
  account: AccountOperationalState;
}) {
  if (account.status !== "paused") return null;
  return (
    <div className="frame" style={{ paddingTop: "var(--aex-space-4)" }}>
          <Notice status="serious" title="This account is paused" live>
            <p className="small">
              {account.reason === "top_up_required"
                ? "The prepaid balance ran out, so new work is declined."
                : "New work is declined."}{" "}
              {account.minimumRestoreCents
                ? `A top-up of at least ${centsToUsd(account.minimumRestoreCents)} restores service.`
                : ""}
            </p>
            <p><a className="button" data-variant="primary" href="/billing">Restore service</a></p>
          </Notice>
    </div>
  );
}

export function AccountBadge({ account }: { account: AccountOperationalState }) {
  return account.status === "active"
    ? <Badge status="good" label="Active" />
    : <Badge status="serious" label="Paused" />;
}
