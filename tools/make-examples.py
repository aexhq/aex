"""Regenerate worked examples for Aex-owned control-plane messages."""

import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "contracts/examples/control"


def write(name, value):
    path = OUT / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
        newline="\n",
    )


wait = {
    "object": "waitlist_entry",
    "email": "dev@example.com",
    "status": "waiting",
    "created_at": "2026-08-18T07:55:00Z",
}
write("JoinWaitlistRequest.example.json", {"email": "dev@example.com"})
write(
    "WaitlistSubmission.example.json",
    {
        "object": "waitlist_submission",
        "email": "dev@example.com",
        "status": "received",
        "received_at": "2026-08-18T07:55:00Z",
    },
)
write("WaitlistEntry.example.json", wait)
write("WaitlistEntryList.example.json", {"object": "list", "data": [wait]})
write("CreateInvitationRequest.example.json", {"email": "dev@example.com"})
write(
    "InvitationCreated.example.json",
    {
        "object": "invitation",
        "email": "dev@example.com",
        "invite_token": "aex_iv_" + "A1b2" * 12,
        "invited_at": "2026-08-18T07:58:00Z",
    },
)

account = {
    "id": "acc_01J5X8Y2K3M4N5P6Q7R8S9T0",
    "object": "account",
    "email": "dev@example.com",
    "created_at": "2026-08-18T08:00:00Z",
    "limits": {"max_concurrent_sessions": 10, "session_creates_per_hour": 30},
}
write("Account.example.json", account)
write(
    "CreateAccountRequest.example.json",
    {"email": "dev@example.com", "invite_token": "aex_iv_" + "A1b2" * 12},
)
write(
    "AccountCreated.example.json",
    {"account": account, "account_token": "aex_at_" + "A1b2" * 12},
)

key = {
    "id": "key_01J5X8Y2K3M4N5P6Q7R8S9U1",
    "object": "api_key",
    "name": "laptop",
    "prefix": "aex_sk_A1b2C",
    "created_at": "2026-08-18T08:05:00Z",
    "last_used_at": "2026-08-18T09:40:00Z",
}
write("ApiKey.example.json", key)
write("CreateApiKeyRequest.example.json", {"name": "laptop"})
write(
    "ApiKeyCreated.example.json",
    {
        "key": {name: value for name, value in key.items() if name != "last_used_at"},
        "secret": "aex_sk_" + "A1b2" * 12,
    },
)
write("ApiKeyList.example.json", {"object": "list", "data": [key]})
write(
    "Balance.example.json",
    {
        "object": "balance",
        "microusd": "9989193",
        "usd": "9.98",
        "metered_to": "2026-08-18T09:45:00Z",
    },
)

write("CreateTopupRequest.example.json", {"amount_cents": 1000})
topup = {
    "id": "top_01J5X8Y2K3M4N5P6Q7R8S9V2",
    "object": "topup",
    "amount_cents": 1000,
    "status": "paid",
    "created_at": "2026-08-18T08:01:00Z",
    "paid_at": "2026-08-18T08:02:10Z",
}
write("Topup.paid.json", topup)
write(
    "Topup.pending.json",
    {
        "id": "top_01J5X8Y2K3M4N5P6Q7R8S9V3",
        "object": "topup",
        "amount_cents": 2500,
        "status": "pending",
        "checkout_url": "https://checkout.stripe.com/c/pay/cs_test_a1B2c3",
        "created_at": "2026-08-18T09:50:00Z",
    },
)
write("TopupList.example.json", {"object": "list", "data": [topup]})
write(
    "CreateCreditGrantRequest.example.json",
    {
        "email": account["email"],
        "amount_cents": 1000,
        "reason": "Alpha evaluation credit",
    },
)
write(
    "CreditGrant.example.json",
    {
        "id": "grt_01J5X8Y2K3M4N5P6Q7R8S9X5",
        "object": "credit_grant",
        "account_id": account["id"],
        "email": account["email"],
        "amount_cents": 1000,
        "reason": "Alpha evaluation credit",
        "created_at": "2026-08-18T09:55:00Z",
    },
)
write("CreateRefundRequest.example.json", {"topup_id": topup["id"], "amount_cents": 500})
write(
    "Refund.example.json",
    {
        "id": "rfd_01J5X8Y2K3M4N5P6Q7R8S9W4",
        "object": "refund",
        "topup_id": topup["id"],
        "amount_cents": 500,
        "status": "succeeded",
        "created_at": "2026-08-18T10:00:00Z",
        "updated_at": "2026-08-18T10:00:01Z",
    },
)

rates = {
    "object": "rate_card",
    "model_gateway": "pass_through",
}
write("RateCard.example.json", rates)
session_usage = {
    "session_id": "ses_01HZX8Y2K3M4N5P6Q7R8S9T0",
    "state": "idle",
    "model_calls": 2,
    "input_tokens": "12450",
    "output_tokens": "980",
    "model_microusd": "129",
    "total_microusd": "129",
    "metered_to": "2026-08-18T09:45:00Z",
}
write("SessionUsage.example.json", session_usage)
write(
    "Usage.example.json",
    {
        "object": "usage",
        "account_id": account["id"],
        "balance_microusd": "9999871",
        "total_microusd": "129",
        "sessions": [session_usage],
        "rates": rates,
        "metered_to": "2026-08-18T09:45:00Z",
    },
)
write(
    "ControlErrorResponse.example.json",
    {
        "error": {
            "code": "insufficient_balance",
            "message": "balance is $0.00; top up at least $10 to run sessions",
            "request_id": "req_ctl_7f",
        }
    },
)

print("control examples written:", len(list(OUT.glob("*.json"))))
