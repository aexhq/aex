import { DevicePanel } from "../../../src/ui/panels/device";

export const dynamic = "force-dynamic";
export const metadata = { title: "Approve a device — AEX" };

/**
 * The page `AEX_CENTRAL_IDENTITY_DEVICE_VERIFICATION_URI` points at.
 *
 * `device_authorization_create` returns this URL twice — bare, and with the
 * code already in the query string — so a person can either click through or
 * type the code they are looking at. This page is the only surface in the
 * product that can complete a CLI login.
 *
 * It sits inside `(app)`, so an unauthenticated visitor is sent to sign in
 * first and arrives back here with their code intact. That ordering is the
 * point: the approval is attributed to the browser session that proved the
 * approver was signed in when they approved.
 */
export default async function DevicePage({
  searchParams,
}: {
  searchParams: Promise<{ user_code?: string }>;
}) {
  const { user_code: userCode } = await searchParams;

  return (
    <div className="stack">
      <div className="page-head">
        <h1>Approve a device</h1>
        <p className="small muted">
          A command-line client is asking to act as you. Check the code matches the one in your
          terminal before you approve it.
        </p>
      </div>
      <DevicePanel prefill={userCode} />
    </div>
  );
}
