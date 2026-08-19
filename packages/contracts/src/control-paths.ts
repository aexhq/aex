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
        /** Join the Founding Beta waitlist (idempotent and privacy-preserving) */
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
        /** The bill — every session's rated line on the two-rate card, storage meters included */
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
        /** The active rate card (public) */
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
        /** @description Operator view of one canonical Founding Beta waitlist record. */
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
        /** @description One-time Founding Beta invitation. Shown once to the operator; only a hash is stored. */
        InvitationToken: string;
        /** @description The invitation token appears here and never again. Creating another invitation rotates it. */
        InvitationCreated: {
            /** @constant */
            object: "invitation";
            email: string;
            invite_token: components["schemas"]["InvitationToken"];
            invited_at: components["schemas"]["Timestamp"];
        };
        CreateAccountRequest: {
            email: string;
            invite_token: components["schemas"]["InvitationToken"];
        };
        AccountId: string;
        /** @description Abuse controls (ARCHITECTURE-v1 §2.9): card + minimum top-up, concurrency and create-rate caps. */
        AccountLimits: {
            max_concurrent_sessions: number;
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
        /** @description Integer micro-USD; 1 USD = 1,000,000. Never a float. */
        MicroUsd: number;
        /** @description Prepaid balance = credits minus rated usage, metered up to `metered_to`. May be negative: usage is rated after the fact; new sessions and messages are refused while it is not positive. */
        Balance: {
            /** @constant */
            object: "balance";
            microusd: components["schemas"]["MicroUsd"];
            /** @description Display form, whole cents, rounded toward zero. */
            usd: string;
            metered_to: components["schemas"]["Timestamp"];
        };
        TopupId: string;
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
            /** @description Whole cents. Founding Beta top-ups are $10.00 to $1,000.00. */
            amount_cents: number;
        };
        /** @description Current stored bytes, as last reported by the brain (session/v1 StorageInfo). */
        StorageMeters: {
            workspace_bytes: number;
            suspended_bytes: number;
            artifact_bytes: number;
        };
        /** @description One session's rated line. Compute time is the sum of turn intervals (turn.started to turn.completed/failed) folded from the session's event log — the journal is the billing record. Storage integrals are exact byte-seconds of the brain-reported meters, piecewise-constant between meter readings. Successful web_search tool results are counted from the same event log. */
        SessionUsage: {
            session_id: string;
            /** @description HandShape from session/v1 (1gb | 2gb | 4gb | 8gb). */
            shape: string;
            /** @description SessionState from session/v1 (active | idle | deleted | failed). */
            state: string;
            running_ms: number;
            suspended_byte_seconds: number;
            workspace_byte_seconds: number;
            artifact_byte_seconds: number;
            web_search_queries: number;
            compute_microusd: components["schemas"]["MicroUsd"];
            storage_microusd: components["schemas"]["MicroUsd"];
            web_search_microusd: components["schemas"]["MicroUsd"];
            total_microusd: components["schemas"]["MicroUsd"];
            storage: components["schemas"]["StorageMeters"];
            metered_to: components["schemas"]["Timestamp"];
        };
        /** @description The two-rate card (ARCHITECTURE-v1 D4). Compute is billed per second while running on the shape's BASELINE (vCPU = memory/2; bursts are free); the pre-suspend idle window is absorbed. Suspended storage covers the bytes the substrate holds for a suspended hand; workspace storage covers synced workspace objects AND persisted artifacts. GB is decimal (1e9 bytes); a month is `month_hours` hours. */
        RateCard: {
            /** @constant */
            object: "rate_card";
            vcpu_hour_microusd: components["schemas"]["MicroUsd"];
            gb_hour_microusd: components["schemas"]["MicroUsd"];
            suspended_gb_month_microusd: components["schemas"]["MicroUsd"];
            workspace_gb_month_microusd: components["schemas"]["MicroUsd"];
            web_search_query_microusd: components["schemas"]["MicroUsd"];
            month_hours: number;
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
    parameters: never;
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
