import { BillingPanel } from "../../../src/ui/panels/billing";

export const dynamic = "force-dynamic";
export const metadata = { title: "Billing — AEX" };

export default function BillingPage() {
  return <main id="main" className="frame"><div className="stack"><div className="page-head"><h1>Billing</h1><p className="small muted">Cards, prepaid balance, top-ups, transactions and usage.</p></div><BillingPanel /></div></main>;
}
