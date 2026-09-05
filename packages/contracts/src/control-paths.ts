/* eslint-disable */
/**
 * GENERATED from contracts/control/v1/openapi.yaml by packages/contracts/scripts/gen.mjs (tools/gen.sh). DO NOT EDIT.
 */
export type paths = {
    "/v1/waitlist": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Join the alpha waitlist (idempotent and privacy-preserving) */
        post: operations["joinWaitlist"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/admin/waitlist": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** List the canonical waitlist, newest first */
        get: operations["listWaitlist"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/admin/invitations": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Create or rotate a one-time invitation for a waitlisted email */
        post: operations["createInvitation"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/admin/credit-grants": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Grant service credit to an existing account
         * @description Operator-only. Appends an auditable, non-payment credit to the account ledger.
         *     Retrying the same Idempotency-Key with the same email, amount and reason returns
         *     the original grant; changing any field returns 409. Grants are not Stripe top-ups
         *     and cannot be refunded through the top-up refund operation.
         */
        post: operations["createCreditGrant"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/admin/refunds": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /**
         * Return unused prepaid credit from a paid top-up
         * @description Operator-only. Aex reserves the requested credit before contacting the payment
         *     provider. Retry an uncertain response with the same Idempotency-Key; using that key
         *     with different request fields returns 409. A failed provider refund restores the
         *     reservation.
         */
        post: operations["createRefund"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/accounts": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Accept a one-time invitation and sign up (returns the account token, shown once) */
        post: operations["createAccount"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/account": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Get the authenticated account */
        get: operations["getAccount"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/keys": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** List API keys (never the secrets) */
        get: operations["listApiKeys"];
        put?: never;
        /** Create an API key (the secret is shown once) */
        post: operations["createApiKey"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/keys/{key_id}": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                key_id: components["schemas"]["KeyId"];
            };
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /** Revoke an API key (immediate; running sessions keep running, new requests with it fail) */
        delete: operations["revokeApiKey"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/balance": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Get the prepaid balance (meters usage up to now first) */
        get: operations["getBalance"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/topups": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** List top-ups (newest first; refreshes pending ones against the payment provider) */
        get: operations["listTopups"];
        put?: never;
        /** Start a top-up; pay at the returned checkout URL */
        post: operations["createTopup"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/topups/{topup_id}": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                topup_id: components["schemas"]["TopupId"];
            };
            cookie?: never;
        };
        /** Get a top-up (polls the payment provider while pending; credits idempotently on paid) */
        get: operations["getTopup"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/topups/checkout/{checkout_session_id}": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                checkout_session_id: string;
            };
            cookie?: never;
        };
        /**
         * Confirm a Stripe Checkout return and reconcile its top-up
         * @description Stripe substitutes the unguessable Checkout Session ID into the configured success URL.
         *     This credential-free endpoint discloses no account or payment details: only the HTTP
         *     status. Webhooks remain the primary settlement path; this poll is an idempotent recovery
         *     path and lets the browser render an authoritative confirmation.
         */
        get: operations["getCheckoutReturn"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/webhooks/stripe": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** Receive a signed Stripe Checkout settlement event */
        post: operations["receiveStripeWebhook"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/usage": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** Exact model-gateway cost and token usage folded from Brain session events */
        get: operations["getUsage"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/v1/rates": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** The public pass-through model-gateway rate policy */
        get: operations["getRates"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
};
export type webhooks = Record<string, never>;
export type components = {
    schemas: {
        JoinWaitlistRequest: {
            email: string;
        };
        /** @enum {string} */
        ControlErrorCode: "invalid_request" | "unauthorized" | "forbidden" | "not_found" | "conflict" | "insufficient_balance" | "rate_limited" | "payment_error" | "upstream_error" | "internal";
        ControlError: {
            code: components["schemas"]["ControlErrorCode"];
            message: string;
            request_id?: string;
        };
        /** @description Error envelope of the control plane's own endpoints. Proxied session/v1 endpoints keep the session ApiError envelope; the control plane injects only codes that ApiErrorCode already has (insufficient_balance, rate_limited, unauthorized, forbidden, not_found). */
        ControlErrorResponse: {
            error: components["schemas"]["ControlError"];
        };
        /**
         * Format: date-time
         * @description RFC 3339, UTC.
         */
        Timestamp: string;
        /** @description A privacy-preserving acknowledgement. It does not reveal whether the email was already waiting, invited, or joined. */
        WaitlistSubmission: {
            /** @constant */
            object: "waitlist_submission";
            email: string;
            /** @constant */
            status: "received";
            received_at: components["schemas"]["Timestamp"];
        };
        /** @enum {string} */
        WaitlistStatus: "waiting" | "invited" | "joined";
        /** @description Operator view of one canonical alpha waitlist record. */
        WaitlistEntry: {
            /** @constant */
            object: "waitlist_entry";
            email: string;
            status: components["schemas"]["WaitlistStatus"];
            created_at: components["schemas"]["Timestamp"];
            invited_at?: components["schemas"]["Timestamp"];
            joined_at?: components["schemas"]["Timestamp"];
        };
        WaitlistEntryList: {
            /** @constant */
            object: "list";
            data: components["schemas"]["WaitlistEntry"][];
        };
        CreateInvitationRequest: {
            email: string;
        };
        /** @description One-time alpha invitation. Shown once to the operator; only a hash is stored. */
        InvitationToken: string;
        /** @description The invitation token appears here and never again. Creating another invitation rotates it. */
        InvitationCreated: {
            /** @constant */
            object: "invitation";
            email: string;
            invite_token: components["schemas"]["InvitationToken"];
            invited_at: components["schemas"]["Timestamp"];
        };
        CreateCreditGrantRequest: {
            email: string;
            /** @description Whole cents of operator-issued service credit. */
            amount_cents: number;
            /** @description Operator audit reason for this grant. */
            reason: string;
        };
        CreditGrantId: string;
        AccountId: string;
        /** @description An operator-issued service-credit grant. It is not backed by a payment and is not refundable as a top-up. Retrying the same Idempotency-Key returns the same grant. */
        CreditGrant: {
            id: components["schemas"]["CreditGrantId"];
            /** @constant */
            object: "credit_grant";
            account_id: components["schemas"]["AccountId"];
            email: string;
            amount_cents: number;
            reason: string;
            created_at: components["schemas"]["Timestamp"];
        };
        TopupId: string;
        CreateRefundRequest: {
            topup_id: components["schemas"]["TopupId"];
            /** @description Whole cents of unused prepaid credit to return from this top-up. */
            amount_cents: number;
        };
        RefundId: string;
        /** @enum {string} */
        RefundStatus: "pending" | "succeeded" | "failed";
        /** @description An operator-initiated return of unused prepaid credit. Credit is reserved before the payment provider is called. Retrying the same Idempotency-Key returns the same refund. */
        Refund: {
            id: components["schemas"]["RefundId"];
            /** @constant */
            object: "refund";
            topup_id: components["schemas"]["TopupId"];
            amount_cents: number;
            status: components["schemas"]["RefundStatus"];
            created_at: components["schemas"]["Timestamp"];
            updated_at: components["schemas"]["Timestamp"];
            /** @description Operator-facing payment-provider failure detail; present only when status is failed. */
            failure_reason?: string;
        };
        CreateAccountRequest: {
            email: string;
            invite_token: components["schemas"]["InvitationToken"];
        };
        /** @description Account-level limits for resource-bearing root sessions and root-session creation rate. Open, asynchronously ending, failed, and deleting roots consume the concurrent limit until a strong ended projection or physical deletion proves resource release; durable child sessions are bounded by the root's sealed child policy. */
        AccountLimits: {
            /** @description Maximum resource-bearing root sessions in open, ending, failed, or deleting lifecycle. Child sessions do not consume or bypass this account limit. */
            max_concurrent_sessions: number;
            /** @description Maximum root-session creates per rolling hour. */
            session_creates_per_hour: number;
        };
        Account: {
            id: components["schemas"]["AccountId"];
            /** @constant */
            object: "account";
            email: string;
            created_at: components["schemas"]["Timestamp"];
            limits: components["schemas"]["AccountLimits"];
        };
        /** @description Manages the account: keys, top-ups, the bill. Shown once at signup; only a hash is stored. */
        AccountToken: string;
        /** @description The account token appears here and never again. */
        AccountCreated: {
            account: components["schemas"]["Account"];
            account_token: components["schemas"]["AccountToken"];
        };
        KeyId: string;
        ApiKey: {
            id: components["schemas"]["KeyId"];
            /** @constant */
            object: "api_key";
            name: string;
            /** @description First characters of the secret, for recognising a key in a list. Never enough to authenticate. */
            prefix: string;
            created_at: components["schemas"]["Timestamp"];
            last_used_at?: components["schemas"]["Timestamp"];
            revoked_at?: components["schemas"]["Timestamp"];
        };
        ApiKeyList: {
            /** @constant */
            object: "list";
            data: components["schemas"]["ApiKey"][];
        };
        CreateApiKeyRequest: {
            name: string;
        };
        /** @description Runs sessions. Shown once at creation; only a hash is stored. */
        ApiKeySecret: string;
        /** @description The secret appears here and never again. */
        ApiKeyCreated: {
            key: components["schemas"]["ApiKey"];
            secret: components["schemas"]["ApiKeySecret"];
        };
        /** @description Canonical signed decimal-string integer micro-USD; 1 USD = 1,000,000. Parse with arbitrary-precision integer arithmetic such as JavaScript BigInt; never Number or floating point. */
        MicroUsd: string;
        /** @description Prepaid balance = credits minus rated usage, metered up to `metered_to`. May be negative: usage is rated after the fact; new sessions and messages are refused while it is not positive. */
        Balance: {
            /** @constant */
            object: "balance";
            microusd: components["schemas"]["MicroUsd"];
            /** @description Display form, whole cents, rounded toward zero. */
            usd: string;
            metered_to: components["schemas"]["Timestamp"];
        };
        /** @enum {string} */
        TopupStatus: "pending" | "paid" | "expired";
        /** @description A prepaid credit purchase. `checkout_url` is where the customer pays (Stripe Checkout); present while pending. The balance is credited when the payment provider reports it paid — on webhook or on poll, idempotently. */
        Topup: {
            id: components["schemas"]["TopupId"];
            /** @constant */
            object: "topup";
            amount_cents: number;
            status: components["schemas"]["TopupStatus"];
            /** Format: uri */
            checkout_url?: string;
            created_at: components["schemas"]["Timestamp"];
            paid_at?: components["schemas"]["Timestamp"];
        };
        TopupList: {
            /** @constant */
            object: "list";
            data: components["schemas"]["Topup"][];
        };
        CreateTopupRequest: {
            /** @description Whole cents. Alpha top-ups are $10.00 to $1,000.00. */
            amount_cents: number;
        };
        /** @description Canonical unsigned decimal-string integer. Parse with arbitrary-precision integer arithmetic such as JavaScript BigInt; never Number or floating point. */
        UnsignedDecimalInteger: string;
        /** @description One session's model usage, folded exactly once from Brain's ordered model_call_ended Events and their provider cost receipts. */
        SessionUsage: {
            session_id: string;
            /** @description The latest session state observed by the Aex control plane. */
            state: string;
            model_calls: number;
            input_tokens: components["schemas"]["UnsignedDecimalInteger"];
            output_tokens: components["schemas"]["UnsignedDecimalInteger"];
            model_microusd: components["schemas"]["MicroUsd"];
            total_microusd: components["schemas"]["MicroUsd"];
            metered_to: components["schemas"]["Timestamp"];
        };
        /** @description The hosted model gateway is billed at the exact cost reported by the upstream AI Gateway receipt, with no Aex markup. */
        RateCard: {
            /** @constant */
            object: "rate_card";
            /** @constant */
            model_gateway: "pass_through";
        };
        /** @description The bill: every session's rated line, the account balance after them, and the rate card they were rated on. */
        Usage: {
            /** @constant */
            object: "usage";
            account_id: components["schemas"]["AccountId"];
            balance_microusd: components["schemas"]["MicroUsd"];
            total_microusd: components["schemas"]["MicroUsd"];
            sessions: components["schemas"]["SessionUsage"][];
            rates: components["schemas"]["RateCard"];
            metered_to: components["schemas"]["Timestamp"];
        };
    };
    responses: {
        /** @description Error */
        Error: {
            headers: {
                [name: string]: unknown;
            };
            content: {
                "application/json": components["schemas"]["ControlErrorResponse"];
            };
        };
    };
    parameters: {
        /** @description A unique key for one intended operator mutation; reuse it only to retry that request. */
        IdempotencyKey: string;
    };
    requestBodies: never;
    headers: never;
    pathItems: never;
};
export type $defs = Record<string, never>;
export interface operations {
    joinWaitlist: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["JoinWaitlistRequest"];
            };
        };
        responses: {
            /** @description Received, whether or not this email already has a record */
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["WaitlistSubmission"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    listWaitlist: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description OK */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["WaitlistEntryList"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    createInvitation: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["CreateInvitationRequest"];
            };
        };
        responses: {
            /** @description Created; the invitation token is shown once */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["InvitationCreated"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    createCreditGrant: {
        parameters: {
            query?: never;
            header: {
                /** @description A unique key for one intended operator mutation; reuse it only to retry that request. */
                "Idempotency-Key": components["parameters"]["IdempotencyKey"];
            };
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["CreateCreditGrantRequest"];
            };
        };
        responses: {
            /** @description Idempotent replay of an existing grant */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["CreditGrant"];
                };
            };
            /** @description Credit granted */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["CreditGrant"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    createRefund: {
        parameters: {
            query?: never;
            header: {
                /** @description A unique key for one intended operator mutation; reuse it only to retry that request. */
                "Idempotency-Key": components["parameters"]["IdempotencyKey"];
            };
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["CreateRefundRequest"];
            };
        };
        responses: {
            /** @description Existing or completed request; inspect status */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Refund"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    createAccount: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["CreateAccountRequest"];
            };
        };
        responses: {
            /** @description Created */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["AccountCreated"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    getAccount: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description OK */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Account"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    listApiKeys: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description OK */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiKeyList"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    createApiKey: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["CreateApiKeyRequest"];
            };
        };
        responses: {
            /** @description Created */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiKeyCreated"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    revokeApiKey: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                key_id: components["schemas"]["KeyId"];
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Revoked */
            204: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            default: components["responses"]["Error"];
        };
    };
    getBalance: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description OK */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Balance"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    listTopups: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description OK */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["TopupList"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    createTopup: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["CreateTopupRequest"];
            };
        };
        responses: {
            /** @description Created (status pending until the payment provider reports it paid) */
            201: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Topup"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    getTopup: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                topup_id: components["schemas"]["TopupId"];
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description OK */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Topup"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    getCheckoutReturn: {
        parameters: {
            query?: never;
            header?: never;
            path: {
                checkout_session_id: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Payment paid and prepaid credit reconciled */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Payment is still pending */
            202: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Checkout Session is not an Aex top-up */
            404: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Checkout Session expired */
            410: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            default: components["responses"]["Error"];
        };
    };
    receiveStripeWebhook: {
        parameters: {
            query?: never;
            header: {
                "Stripe-Signature": string;
            };
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": Record<string, never>;
            };
        };
        responses: {
            /** @description Event verified and accepted (including an irrelevant event type) */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Invalid signature, stale timestamp, or malformed event */
            400: {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
        };
    };
    getUsage: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description OK */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Usage"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
    getRates: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description OK */
            200: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RateCard"];
                };
            };
            default: components["responses"]["Error"];
        };
    };
}
