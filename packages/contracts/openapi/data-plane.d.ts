/**
 * GENERATED FILE — DO NOT EDIT.
 *
 * Emitted by `bun run openapi:types:generate` from ./data-plane.json with
 * openapi-typescript. That document is itself generated from the schemas in
 * packages/contracts/src/schemas/** and the route table in src/api-routes.ts,
 * so this file is two derivations away from the code the server runs and zero
 * derivations away from anything hand-maintained.
 *
 * `openapi:types:check` fails when this file and a fresh generation disagree.
 */
export interface paths {
    "/api/admin/billing/account-type": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** adminBilling.accountType */
        post: operations["adminBilling.accountType"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/admin/billing/payment-method": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** adminBilling.paymentMethod */
        post: operations["adminBilling.paymentMethod"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/admin/billing/topup": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** adminBilling.topup */
        post: operations["adminBilling.topup"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/assets/{assetId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /** assets.delete */
        delete: operations["assets.delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/assets/finalize": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** assets.finalize */
        post: operations["assets.finalize"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/assets/mpu/abort": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** assets.mpuAbort */
        post: operations["assets.mpuAbort"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/assets/mpu/presign-parts": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** assets.mpuPresignParts */
        post: operations["assets.mpuPresignParts"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/assets/presign": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** assets.presign */
        post: operations["assets.presign"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/billing": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** billing.get */
        get: operations["billing.get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/billing/ledger": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** billing.ledger */
        get: operations["billing.ledger"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/billing/portal": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** billing.portal */
        post: operations["billing.portal"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/internal/sessions/{sessionId}/archive": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** sessions.archiveInternal */
        get: operations["sessions.archiveInternal"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/mcp-servers": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** mcpServers.list */
        get: operations["mcpServers.list"];
        put?: never;
        /** mcpServers.create */
        post: operations["mcpServers.create"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/mcp-servers/{mcpServerId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** mcpServers.get */
        get: operations["mcpServers.get"];
        put?: never;
        post?: never;
        /** mcpServers.delete */
        delete: operations["mcpServers.delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/runtime/journal/commit": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** runtime.journalCommit */
        post: operations["runtime.journalCommit"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/runtime/writer-authority": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** runtime.writerAuthority */
        get: operations["runtime.writerAuthority"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/secrets": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** secrets.list */
        get: operations["secrets.list"];
        put?: never;
        /** secrets.create */
        post: operations["secrets.create"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/secrets/{secretId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** secrets.get */
        get: operations["secrets.get"];
        put?: never;
        post?: never;
        /** secrets.delete */
        delete: operations["secrets.delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/secrets/{secretId}/rotate": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** secrets.rotate */
        post: operations["secrets.rotate"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** sessions.list */
        get: operations["sessions.list"];
        put?: never;
        /** sessions.create */
        post: operations["sessions.create"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** sessions.get */
        get: operations["sessions.get"];
        put?: never;
        post?: never;
        /** sessions.delete */
        delete: operations["sessions.delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/approve": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** sessions.approve */
        post: operations["sessions.approve"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/cancel": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** sessions.cancel */
        post: operations["sessions.cancel"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/children": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** sessions.listChildren */
        get: operations["sessions.listChildren"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/deny": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** sessions.deny */
        post: operations["sessions.deny"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/events": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** sessions.listEvents */
        get: operations["sessions.listEvents"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/events/link": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** sessions.eventArchiveLink */
        post: operations["sessions.eventArchiveLink"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/events/ticket": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** sessions.eventsTicket */
        post: operations["sessions.eventsTicket"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/files": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** sessions.listFiles */
        get: operations["sessions.listFiles"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/files/{fileId}/download": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** sessions.downloadFile */
        get: operations["sessions.downloadFile"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/files/{fileId}/link": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** sessions.fileLink */
        post: operations["sessions.fileLink"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/finalize": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** sessions.finalize */
        post: operations["sessions.finalize"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/messages": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** sessions.listMessages */
        get: operations["sessions.listMessages"];
        put?: never;
        /** sessions.sendMessage */
        post: operations["sessions.sendMessage"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/otel": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** sessions.otel */
        get: operations["sessions.otel"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/request-approval": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** sessions.requestApproval */
        post: operations["sessions.requestApproval"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/result": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** sessions.childResult */
        get: operations["sessions.childResult"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/resume": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** sessions.resume */
        post: operations["sessions.resume"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/suspend": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** sessions.suspend */
        post: operations["sessions.suspend"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/webhook-deliveries": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** sessions.listWebhookDeliveries */
        get: operations["sessions.listWebhookDeliveries"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/webhook-deliveries/{webhookDeliveryId}/redeliver": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** sessions.redeliverWebhook */
        post: operations["sessions.redeliverWebhook"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/webhook/deliveries": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** webhook.listDeliveries */
        get: operations["webhook.listDeliveries"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/webhook/signing-secret": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** webhook.signingSecret */
        post: operations["webhook.signingSecret"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/whoami": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** whoami */
        get: operations["whoami"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/files": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** workspace.files.list */
        get: operations["workspace.files.list"];
        put?: never;
        /** workspace.files.publish */
        post: operations["workspace.files.publish"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/files/{fileId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** workspace.files.get */
        get: operations["workspace.files.get"];
        put?: never;
        post?: never;
        /** workspace.files.delete */
        delete: operations["workspace.files.delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/instructions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** workspace.instructions.list */
        get: operations["workspace.instructions.list"];
        put?: never;
        /** workspace.instructions.publish */
        post: operations["workspace.instructions.publish"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/instructions/{instructionId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** workspace.instructions.get */
        get: operations["workspace.instructions.get"];
        put?: never;
        post?: never;
        /** workspace.instructions.delete */
        delete: operations["workspace.instructions.delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/skills": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** workspace.skills.list */
        get: operations["workspace.skills.list"];
        put?: never;
        /** workspace.skills.publish */
        post: operations["workspace.skills.publish"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/skills/{skillId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** workspace.skills.get */
        get: operations["workspace.skills.get"];
        put?: never;
        post?: never;
        /** workspace.skills.delete */
        delete: operations["workspace.skills.delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/tools": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** workspace.tools.list */
        get: operations["workspace.tools.list"];
        put?: never;
        /** workspace.tools.publish */
        post: operations["workspace.tools.publish"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/tools/{toolId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** workspace.tools.get */
        get: operations["workspace.tools.get"];
        put?: never;
        post?: never;
        /** workspace.tools.delete */
        delete: operations["workspace.tools.delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspaces/{workspaceId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /** workspaces.erase */
        delete: operations["workspaces.erase"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
}
export type webhooks = Record<string, never>;
export interface components {
    schemas: {
        ApiErrorEnvelope: {
            /** @description Stable machine-readable error code. */
            error: string;
            message: string;
            requestId?: string;
        } & {
            [key: string]: unknown;
        };
        /** @description The vaulted half of a submission. Excluded from the idempotency hash and never echoed back. */
        InlineSecrets: {
            mcpServers?: components["schemas"]["SecretsMcpServers"];
            envSecrets?: components["schemas"]["SecretsEnvSecrets"];
        };
        /** @description Per-session env-var secret values. Keys must be valid env var names; each pairs with a `submission.secretEnv` declaration. */
        SecretsEnvSecrets: {
            [key: string]: unknown;
        };
        /** @description Per-session MCP server credentials. Server names must be unique. */
        SecretsMcpServers: {
            name: string;
            url: string;
            headers?: {
                [key: string]: string;
            };
        }[];
        /** @description Per-session lineage-limit override. Shape and positivity only; clamping to the workspace and platform ceilings happens server-side in resolveSessionLimits. */
        SessionLimits: {
            maxConcurrentChildSessions?: number;
            maxSubagentDepth?: number;
            maxSpendUsd?: number;
            maxTurns?: number;
            maxStepsPerTurn?: number;
        };
        /** @description Capacity intent. An object with no `spot` carries no signal and is dropped; `spot: false` is preserved as an explicit request for standard capacity. */
        SessionMachine: {
            spot?: boolean;
        };
        SessionSubmissionRequest: {
            workspaceId: string;
            idempotencyKey: string;
            submission: components["schemas"]["Submission"];
            /** @enum {string} */
            runtimeSize?: "0.25cpu-1gb" | "0.5cpu-4gb" | "1cpu-6gb" | "2cpu-8gb" | "4cpu-12gb";
            /** @enum {string} */
            runtimeKind?: "container" | "spot_container" | "lambda";
            timeout?: string;
            webhook?: components["schemas"]["SessionWebhook"];
            limits?: components["schemas"]["SessionLimits"];
            machine?: components["schemas"]["SessionMachine"];
            secrets?: components["schemas"]["InlineSecrets"] | null;
        };
        /** @description Run-callback registration. `url` must be https with no userinfo — enforced in packages/contracts/src/schemas/session-webhook.ts, and not expressible in JSON Schema. */
        SessionWebhook: {
            url: string;
        };
        Submission: {
            model: string;
            system?: string;
            prompt: components["schemas"]["SubmissionPrompt"];
            assets: components["schemas"]["SubmissionAssets"];
            mcpServers?: components["schemas"]["SubmissionMcpServers"];
            secretEnv?: components["schemas"]["SubmissionSecretEnv"] | null;
            environment?: components["schemas"]["SubmissionEnvironment"];
            securityProfile?: ("strict" | "standard" | "developer") | null;
            metadata?: {
                [key: string]: unknown;
            };
            fileCapture?: components["schemas"]["SubmissionFileCapture"] | null;
            builtinTools?: ("default" | "none" | unknown[]) | null;
            outputMode?: ("buffered" | "stream") | null;
            responseFormat?: components["schemas"]["SubmissionResponseFormat"] | null;
            approvalGate?: components["schemas"]["SubmissionApprovalGate"] | null;
            platform?: components["schemas"]["SubmissionPlatformInjection"] | null;
        };
        SubmissionApprovalGate: {
            tools: string[];
        };
        SubmissionAssets: {
            files?: {
                /** @constant */
                kind: "file";
                resourceId: string;
                version: number;
                assetId: string;
                contentHash: string;
                name: string;
                mountPath: string;
            }[];
            skills?: {
                /** @constant */
                kind: "skill";
                resourceId: string;
                version: number;
                assetId: string;
                contentHash: string;
                name: string;
                description: string;
            }[];
            tools?: {
                /** @constant */
                kind: "tool";
                resourceId: string;
                version: number;
                assetId: string;
                contentHash: string;
                name: string;
                description: string;
                input_schema: {
                    [key: string]: unknown;
                };
                entry: string;
            }[];
            instructions?: {
                /** @constant */
                kind: "instruction";
                resourceId: string;
                version: number;
                assetId: string;
                contentHash: string;
                name: string;
            }[];
        };
        /** @description Customer-controlled runtime environment. */
        SubmissionEnvironment: {
            networking?: components["schemas"]["SubmissionNetworking"];
            packages?: components["schemas"]["SubmissionPackages"];
            envVars?: components["schemas"]["SubmissionEnvVars"];
        };
        SubmissionEnvVars: {
            [key: string]: unknown;
        };
        SubmissionFileCapture: {
            allowedDirs?: unknown[];
            deniedDirs?: unknown[];
            captureTimeoutMs?: number;
            maxFileBytes?: number;
            maxTotalBytes?: number;
            maxFiles?: number;
        };
        /** @description Remote MCP servers, each `{ name, url, transport? }`. Names must be unique and every URL must clear the SSRF host deny-list — enforced in packages/contracts/src/session-config.ts. */
        SubmissionMcpServers: unknown[];
        /** @description Egress policy. `mode` is required whenever `networking` is supplied — enforced in packages/contracts/src/schemas/submission-environment.ts. */
        SubmissionNetworking: {
            /** @enum {string} */
            mode?: "limited" | "open";
            allowedHosts?: string[];
        };
        /** @description Package request. `name` may carry an ecosystem prefix ("pip:pandas"); an unprefixed name defaults to apt and an unknown prefix is rejected. */
        SubmissionPackage: {
            name: string;
            version?: string;
        };
        SubmissionPackages: components["schemas"]["SubmissionPackage"][];
        SubmissionPlatformInjection: {
            /** @enum {string} */
            systemPrompt?: "default" | "off";
        };
        /** @description The run brief: one string, or an ordered list of non-empty parts. At least one part must carry non-whitespace text. A single string is normalised to a one-element list. */
        SubmissionPrompt: string | unknown[];
        SubmissionResponseFormat: {
            /** @constant */
            kind: "text";
        } | {
            /** @constant */
            kind: "json_schema";
            schema: {
                [key: string]: unknown;
            };
            strict?: boolean;
            name?: string;
        };
        /** @description Env-var secret bindings keyed by env name. Each value is exactly one of `{ ref }` (a workspace secret handle matching ^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$) or `{ ephemeral: true }` (the value rides in `secrets.envSecrets`). Enforced in packages/contracts/src/schemas/submission-body.ts. */
        SubmissionSecretEnv: {
            [key: string]: unknown;
        };
    };
    responses: never;
    parameters: never;
    requestBodies: never;
    headers: never;
    pathItems: never;
}
export type $defs = Record<string, never>;
export interface operations {
    "adminBilling.accountType": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "adminBilling.paymentMethod": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "adminBilling.topup": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "assets.delete": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                assetId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "assets.finalize": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "assets.mpuAbort": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "assets.mpuPresignParts": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "assets.presign": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "billing.get": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "billing.ledger": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "billing.portal": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.archiveInternal": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "mcpServers.list": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "mcpServers.create": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "mcpServers.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                mcpServerId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "mcpServers.delete": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                mcpServerId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "runtime.journalCommit": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "runtime.writerAuthority": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "secrets.list": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "secrets.create": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "secrets.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                secretId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "secrets.delete": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                secretId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "secrets.rotate": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                secretId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.list": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.create": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["SessionSubmissionRequest"];
            };
        };
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.delete": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.approve": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.cancel": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.listChildren": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.deny": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.listEvents": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.eventArchiveLink": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.eventsTicket": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.listFiles": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.downloadFile": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
                fileId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.fileLink": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
                fileId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.finalize": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.listMessages": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.sendMessage": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.otel": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.requestApproval": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.childResult": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.resume": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.suspend": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.listWebhookDeliveries": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "sessions.redeliverWebhook": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
                webhookDeliveryId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "webhook.listDeliveries": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "webhook.signingSecret": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    whoami: {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "workspace.files.list": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "workspace.files.publish": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "workspace.files.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                fileId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "workspace.files.delete": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                fileId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "workspace.instructions.list": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "workspace.instructions.publish": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "workspace.instructions.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                instructionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "workspace.instructions.delete": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                instructionId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "workspace.skills.list": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "workspace.skills.publish": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "workspace.skills.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                skillId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "workspace.skills.delete": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                skillId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "workspace.tools.list": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "workspace.tools.publish": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "workspace.tools.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                toolId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "workspace.tools.delete": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                toolId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
    "workspaces.erase": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                workspaceId: string;
            };
            cookie?: never;
        };
        requestBody?: never;
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content?: never;
            };
            /** @description Error envelope. */
            default: {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["ApiErrorEnvelope"];
                };
            };
        };
    };
}
