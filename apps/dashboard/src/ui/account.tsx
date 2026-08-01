import type { OperationalState, Organization } from "../server/bootstrap";
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
  organizations,
}: {
  account: OperationalState;
  organizations: readonly Organization[];
}) {
  if (account.status === "active") return null;
  const billing = organizations[0];
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
        {account.retentionFundedUntil ? (
          <p className="small">
            Retained content stays funded until {account.retentionFundedUntil}
            {account.deletionScheduledAt
              ? `, and unfunded content is scheduled for deletion at ${account.deletionScheduledAt}.`
              : "."}
          </p>
        ) : null}
        {billing ? (
          <p>
            <a className="button" data-variant="primary" href={`/org/${billing.slug}/billing`}>
              Restore service
            </a>
          </p>
        ) : null}
      </Notice>
    </div>
  );
}

export function AccountBadge({ account }: { account: OperationalState }) {
  return account.status === "active"
    ? <Badge status="good" label="Active" />
    : <Badge status="serious" label="Paused" />;
}
