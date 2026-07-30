/**
 * GENERATED FILE — DO NOT EDIT.
 *
 * Emitted by `bun run openapi:types:generate` from ./regional.json with
 * openapi-typescript. That document is itself generated from the schemas in
 * packages/contracts/src/schemas/** and the route table in src/api-routes.ts,
 * so this file is two derivations away from the code the server runs and zero
 * derivations away from anything hand-maintained.
 *
 * `openapi:types:check` fails when this file and a fresh generation disagree.
 */
export interface paths {
    "/api/operations": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** operations.list */
        get: operations["operations.list"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/operations/{operationId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** operations.get */
        get: operations["operations.get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/operations/{operationId}/cancellations": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** operations.cancel */
        post: operations["operations.cancel"];
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
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/credential-rebinds": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** sessions.credentials.rebind */
        post: operations["sessions.credentials.rebind"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/deletions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** sessions.delete */
        post: operations["sessions.delete"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/forks": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** sessions.fork */
        post: operations["sessions.fork"];
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
        /** messages.list */
        get: operations["messages.list"];
        put?: never;
        /** messages.send */
        post: operations["messages.send"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/persists": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** sessions.persist */
        post: operations["sessions.persist"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/runs": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** runs.list */
        get: operations["runs.list"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/runs/{runId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** runs.get */
        get: operations["runs.get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/stops": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** sessions.stop */
        post: operations["sessions.stop"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/workspace/discards": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** sessions.workspace.discard */
        post: operations["sessions.workspace.discard"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** workspace.get */
        get: operations["workspace.get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
}
export type webhooks = Record<string, never>;
export interface components {
    schemas: {
        ApiError: {
            error: {
                code: string;
                message: string;
                requestId: string;
                retryable: boolean;
                operationId?: string;
                details?: {
                    [key: string]: unknown;
                };
            };
        };
        /** @description The vaulted half of a submission. Excluded from the idempotency hash and never echoed back. */
        InlineSecrets: {
            mcpServers?: components["schemas"]["SecretsMcpServers"];
            envSecrets?: components["schemas"]["SecretsEnvSecrets"];
        };
        Message: {
            id: string;
            sessionId: string;
            runId?: string;
            /** @enum {string} */
            role: "user" | "assistant" | "tool";
            content: ({
                /** @constant */
                type: "text";
                text: string;
            } | {
                /** @constant */
                type: "file";
                path: string;
                /** @constant */
                source: "persisted";
                mediaType?: string;
            })[];
            createdAt: string;
        };
        MessageSendRequest: {
            content: ({
                /** @constant */
                type: "text";
                text: string;
            } | {
                /** @constant */
                type: "file";
                path: string;
                /** @constant */
                source: "persisted";
                mediaType?: string;
            })[];
            maxSpendCents?: number;
        };
        Operation: {
            id: string;
            workspaceId: string;
            sessionId?: string;
            /** @enum {string} */
            status: "queued" | "running" | "succeeded" | "failed" | "cancelled";
            progress?: {
                phase: string;
                completed?: number;
                total?: number;
            };
            cancelable: boolean;
            error?: {
                code: string;
                message: string;
                requestId: string;
                retryable: boolean;
                operationId?: string;
                details?: {
                    [key: string]: unknown;
                };
            };
            createdAt: string;
            startedAt?: string;
            updatedAt: string;
            committedAt?: string;
            terminalAt?: string;
            /** @constant */
            kind: "session_stop";
            result?: {
                sessionId: string;
                changed: boolean;
                sessionRevision: number;
            };
        } | {
            id: string;
            workspaceId: string;
            sessionId?: string;
            /** @enum {string} */
            status: "queued" | "running" | "succeeded" | "failed" | "cancelled";
            progress?: {
                phase: string;
                completed?: number;
                total?: number;
            };
            cancelable: boolean;
            error?: {
                code: string;
                message: string;
                requestId: string;
                retryable: boolean;
                operationId?: string;
                details?: {
                    [key: string]: unknown;
                };
            };
            createdAt: string;
            startedAt?: string;
            updatedAt: string;
            committedAt?: string;
            terminalAt?: string;
            /** @constant */
            kind: "session_persist";
            result?: {
                sessionId: string;
                changed: boolean;
                persistRevision: number;
                rootHash: string;
                lastPersistedAt: string;
                added: number;
                updated: number;
                deleted: number;
                bytesMoved: number;
            };
        } | {
            id: string;
            workspaceId: string;
            sessionId?: string;
            /** @enum {string} */
            status: "queued" | "running" | "succeeded" | "failed" | "cancelled";
            progress?: {
                phase: string;
                completed?: number;
                total?: number;
            };
            cancelable: boolean;
            error?: {
                code: string;
                message: string;
                requestId: string;
                retryable: boolean;
                operationId?: string;
                details?: {
                    [key: string]: unknown;
                };
            };
            createdAt: string;
            startedAt?: string;
            updatedAt: string;
            committedAt?: string;
            terminalAt?: string;
            /** @constant */
            kind: "session_fork";
            result?: {
                session: components["schemas"]["Session"];
            };
        } | {
            id: string;
            workspaceId: string;
            sessionId?: string;
            /** @enum {string} */
            status: "queued" | "running" | "succeeded" | "failed" | "cancelled";
            progress?: {
                phase: string;
                completed?: number;
                total?: number;
            };
            cancelable: boolean;
            error?: {
                code: string;
                message: string;
                requestId: string;
                retryable: boolean;
                operationId?: string;
                details?: {
                    [key: string]: unknown;
                };
            };
            createdAt: string;
            startedAt?: string;
            updatedAt: string;
            committedAt?: string;
            terminalAt?: string;
            /** @constant */
            kind: "workspace_discard";
            result?: {
                sessionId: string;
                changed: boolean;
                continuity: {
                    /** @constant */
                    state: "warm";
                    /** @enum {string} */
                    availability: "running" | "suspended";
                    generationId: string;
                    materializedFromPersistRevision: number;
                    changedAt: string;
                } | {
                    /** @constant */
                    state: "cold";
                    persistedRevision: number;
                    changedAt: string;
                    /** @enum {string} */
                    reason: "not_started" | "idle_retention_elapsed" | "hard_lifetime_reached" | "user_discarded" | "unexpected_loss";
                    previousGenerationId?: string;
                };
            };
        } | {
            id: string;
            workspaceId: string;
            sessionId?: string;
            /** @enum {string} */
            status: "queued" | "running" | "succeeded" | "failed" | "cancelled";
            progress?: {
                phase: string;
                completed?: number;
                total?: number;
            };
            cancelable: boolean;
            error?: {
                code: string;
                message: string;
                requestId: string;
                retryable: boolean;
                operationId?: string;
                details?: {
                    [key: string]: unknown;
                };
            };
            createdAt: string;
            startedAt?: string;
            updatedAt: string;
            committedAt?: string;
            terminalAt?: string;
            /** @constant */
            kind: "session_delete";
            result?: {
                sessionId: string;
                workspaceId: string;
                deletedAt: string;
                deletionOperationId: string;
                residualRetention: {
                    system: string;
                    /** @enum {string} */
                    class: "managed_backup" | "stream" | "provider_media";
                    /** @enum {string} */
                    state: "scheduled" | "verified" | "unknown";
                    purgeBy?: string;
                }[];
            };
        } | {
            id: string;
            workspaceId: string;
            sessionId?: string;
            /** @enum {string} */
            status: "queued" | "running" | "succeeded" | "failed" | "cancelled";
            progress?: {
                phase: string;
                completed?: number;
                total?: number;
            };
            cancelable: boolean;
            error?: {
                code: string;
                message: string;
                requestId: string;
                retryable: boolean;
                operationId?: string;
                details?: {
                    [key: string]: unknown;
                };
            };
            createdAt: string;
            startedAt?: string;
            updatedAt: string;
            committedAt?: string;
            terminalAt?: string;
            /** @constant */
            kind: "credential_rebind";
            result?: {
                sessionId: string;
                custodyRevision: number;
                secrets: {
                    name: string;
                }[];
            };
        } | {
            id: string;
            workspaceId: string;
            sessionId?: string;
            /** @enum {string} */
            status: "queued" | "running" | "succeeded" | "failed" | "cancelled";
            progress?: {
                phase: string;
                completed?: number;
                total?: number;
            };
            cancelable: boolean;
            error?: {
                code: string;
                message: string;
                requestId: string;
                retryable: boolean;
                operationId?: string;
                details?: {
                    [key: string]: unknown;
                };
            };
            createdAt: string;
            startedAt?: string;
            updatedAt: string;
            committedAt?: string;
            terminalAt?: string;
            /** @constant */
            kind: "telemetry_export";
            result?: {
                exportId: string;
                /** @enum {string} */
                format: "ndjson" | "parquet" | "otlp_json";
                manifestHash: string;
                expiresAt: string;
            };
        } | {
            id: string;
            workspaceId: string;
            sessionId?: string;
            /** @enum {string} */
            status: "queued" | "running" | "succeeded" | "failed" | "cancelled";
            progress?: {
                phase: string;
                completed?: number;
                total?: number;
            };
            cancelable: boolean;
            error?: {
                code: string;
                message: string;
                requestId: string;
                retryable: boolean;
                operationId?: string;
                details?: {
                    [key: string]: unknown;
                };
            };
            createdAt: string;
            startedAt?: string;
            updatedAt: string;
            committedAt?: string;
            terminalAt?: string;
            /** @constant */
            kind: "workspace_delete";
            result?: {
                workspaceId: string;
                deletedAt: string;
                deletionOperationId: string;
                residualRetention: {
                    system: string;
                    /** @enum {string} */
                    class: "managed_backup" | "stream" | "provider_media";
                    /** @enum {string} */
                    state: "scheduled" | "verified" | "unknown";
                    purgeBy?: string;
                }[];
            };
        };
        Run: {
            id: string;
            sessionId: string;
            messageId: string;
            /** @enum {string} */
            status: "queued" | "running" | "succeeded" | "failed" | "timed_out" | "cancelled" | "interrupted";
            maxSpendCents: number;
            queuedAt: string;
            startedAt?: string;
            terminalAt?: string;
            outputMessageIds?: string[];
            error?: {
                code: string;
                message: string;
                requestId: string;
                retryable: boolean;
                operationId?: string;
                details?: {
                    [key: string]: unknown;
                };
            };
            telemetryComplete?: boolean;
            telemetryRejectionIds?: string[];
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
        Session: {
            id: string;
            workspaceId: string;
            /** @enum {string} */
            status: "idle" | "running" | "awaiting_approval" | "deleting";
            revision: number;
            persistRevision: number;
            createdAt: string;
            updatedAt: string;
            lastPersistedAt?: string;
            continuity: {
                /** @constant */
                state: "warm";
                /** @enum {string} */
                availability: "running" | "suspended";
                generationId: string;
                materializedFromPersistRevision: number;
                changedAt: string;
            } | {
                /** @constant */
                state: "cold";
                persistedRevision: number;
                changedAt: string;
                /** @enum {string} */
                reason: "not_started" | "idle_retention_elapsed" | "hard_lifetime_reached" | "user_discarded" | "unexpected_loss";
                previousGenerationId?: string;
            };
            lineage: {
                parentSessionId?: string;
                forkedAt?: string;
            };
            resolvedConfig: {
                builtinCatalogHash: string;
                builtinTools: ("bash" | "read_file" | "write_file" | "edit_file" | "grep" | "glob" | "head" | "tail" | "todo_write" | "subagent" | "subagent_result" | "web_fetch" | "web_search" | "bash_output" | "bash_kill" | "code_execution" | "wait" | "git" | "ls" | "stat" | "wc")[];
                approvalPolicy: {
                    /** @constant */
                    mode: "allow_all";
                } | {
                    /** @constant */
                    mode: "require_for_tools";
                    tools: string[];
                };
                network: {
                    hands: {
                        /** @enum {string} */
                        mode: "none" | "public_internet";
                    };
                };
                packages: {
                    /** @enum {string} */
                    ecosystem: "apt" | "pip" | "npm";
                    name: string;
                    version: string;
                }[];
                compute: {
                    /** @enum {string} */
                    requestedSize: "512mb" | "1gb" | "2gb" | "4gb" | "8gb";
                    /** @enum {string} */
                    peakSize: "512mb" | "1gb" | "2gb" | "4gb" | "8gb";
                    /** @enum {string} */
                    diskSize: "8gb" | "16gb" | "32gb";
                };
                continuityPolicy: {
                    /** @constant */
                    idleAction: "hibernate";
                    /** @constant */
                    idleDelayMs: 180000;
                    /** @constant */
                    warmRetention: "provider_lifetime";
                    /** @constant */
                    hardLifetimeMs: 28800000;
                };
                harness: {
                    /** @constant */
                    protocol: "aex-agent-v1";
                    revision: string;
                    /** @constant */
                    platformPrompt: "required";
                    /** @constant */
                    instructionDiscovery: "none";
                    /** @constant */
                    toolResultContextLimitBytes: 65536;
                };
            };
        };
        SessionCreateRequestV1: {
            model: string;
            registered?: {
                files?: string[];
                skills?: string[];
                tools?: string[];
                instructions?: string[];
                mcpServers?: string[];
            };
            credentials?: {
                secrets: {
                    name: string;
                }[];
            };
            compute?: {
                /** @enum {string} */
                requestedSize?: "512mb" | "1gb" | "2gb" | "4gb" | "8gb";
                /** @enum {string} */
                peakSize?: "512mb" | "1gb" | "2gb" | "4gb" | "8gb";
                /** @enum {string} */
                diskSize?: "8gb" | "16gb" | "32gb";
            };
            network?: {
                hands: {
                    /** @enum {string} */
                    mode: "none" | "public_internet";
                };
            };
            packages?: {
                /** @enum {string} */
                ecosystem: "apt" | "pip" | "npm";
                name: string;
                version: string;
            }[];
            approvalPolicy?: {
                /** @constant */
                mode: "allow_all";
            } | {
                /** @constant */
                mode: "require_for_tools";
                tools: string[];
            };
            metadata?: {
                [key: string]: string | number | boolean | null;
            };
        };
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
                textHash: string;
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
        WorkspaceApiKeyValue: string;
    };
    responses: never;
    parameters: never;
    requestBodies: never;
    headers: never;
    pathItems: never;
}
export type $defs = Record<string, never>;
export interface operations {
    "operations.list": {
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "operations.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                operationId: string;
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "operations.cancel": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                operationId: string;
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
                    "application/json": components["schemas"]["ApiError"];
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "sessions.create": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["SessionCreateRequestV1"];
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
                    "application/json": components["schemas"]["ApiError"];
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "sessions.credentials.rebind": {
        parameters: {
            query?: never;
            header: {
                "Aex-Operation-Id": string;
            };
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "sessions.delete": {
        parameters: {
            query?: never;
            header: {
                "Aex-Operation-Id": string;
            };
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "sessions.fork": {
        parameters: {
            query?: never;
            header: {
                "Aex-Operation-Id": string;
            };
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "messages.list": {
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "messages.send": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["MessageSendRequest"];
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "sessions.persist": {
        parameters: {
            query?: never;
            header: {
                "Aex-Operation-Id": string;
            };
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "runs.list": {
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "runs.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
                runId: string;
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "sessions.stop": {
        parameters: {
            query?: never;
            header: {
                "Aex-Operation-Id": string;
            };
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "sessions.workspace.discard": {
        parameters: {
            query?: never;
            header: {
                "Aex-Operation-Id": string;
            };
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "workspace.get": {
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
}
