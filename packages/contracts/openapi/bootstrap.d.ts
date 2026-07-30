/**
 * GENERATED FILE — DO NOT EDIT.
 *
 * Emitted by `bun run openapi:types:generate` from ./bootstrap.json with
 * openapi-typescript. That document is itself generated from the schemas in
 * packages/contracts/src/schemas/** and the route table in src/api-routes.ts,
 * so this file is two derivations away from the code the server runs and zero
 * derivations away from anything hand-maintained.
 *
 * `openapi:types:check` fails when this file and a fresh generation disagree.
 */
export interface paths {
    "/api/account": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** account.get */
        get: operations["account.get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/api-keys": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** apiKeys.list */
        get: operations["apiKeys.list"];
        put?: never;
        /** apiKeys.create */
        post: operations["apiKeys.create"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/api-keys/{apiKeyId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /** apiKeys.delete */
        delete: operations["apiKeys.delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/billing/balance": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** billing.balance */
        get: operations["billing.balance"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
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
    "/api/organizations": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** organizations.list */
        get: operations["organizations.list"];
        put?: never;
        /** organizations.create */
        post: operations["organizations.create"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/organizations/{organizationId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** organizations.get */
        get: operations["organizations.get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/organizations/{organizationId}/billing/auto-topup-policy": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** billing.autoTopup.get */
        get: operations["billing.autoTopup.get"];
        /** billing.autoTopup.put */
        put: operations["billing.autoTopup.put"];
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/organizations/{organizationId}/billing/portal-sessions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** billing.portalSession */
        post: operations["billing.portalSession"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/organizations/{organizationId}/billing/statements": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** billing.statements.list */
        get: operations["billing.statements.list"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/organizations/{organizationId}/billing/statements/{statementId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** billing.statements.get */
        get: operations["billing.statements.get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/organizations/{organizationId}/billing/statements/{statementId}/downloads": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** billing.statements.download */
        post: operations["billing.statements.download"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/organizations/{organizationId}/billing/top-up-checkouts": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** billing.topUpCheckout */
        post: operations["billing.topUpCheckout"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/organizations/{organizationId}/invitations": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** invitations.create */
        post: operations["invitations.create"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/organizations/{organizationId}/memberships": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** memberships.list */
        get: operations["memberships.list"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspaces": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** workspaces.list */
        get: operations["workspaces.list"];
        put?: never;
        /** workspaces.create */
        post: operations["workspaces.create"];
        delete?: never;
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
        /** workspaces.get */
        get: operations["workspaces.get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspaces/{workspaceId}/deletions": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** workspaces.delete */
        post: operations["workspaces.delete"];
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
        ApiKeyCreateRequest: {
            workspaceId: string;
            name: string;
            scopes: string[];
        };
        ApprovalResponseRequest: {
            /** @enum {string} */
            decision: "approve" | "deny";
        };
        AutoTopupPolicyRequest: {
            enabled: boolean;
            thresholdUsd: number;
            amountUsd: number;
        };
        BlobDescriptor: {
            sha256: string;
            sizeBytes: number;
        };
        DownloadGrant: {
            url: string;
            headers?: {
                [key: string]: string;
            };
            expiresAt: string;
            sizeBytes: number;
            authorizedBytes: number;
            measurementId: string;
            sha256: string;
        };
        EffectiveWorkspaceLimit: {
            id: string;
            effectiveValue: number | {
                [key: string]: number;
            };
            /** @enum {string} */
            source: "default" | "workspace_override";
            /** @constant */
            adjustable: true;
            revision: number;
            changedAt: string;
        };
        EffectiveWorkspaceLimitPage: {
            items: components["schemas"]["EffectiveWorkspaceLimit"][];
            nextCursor?: string;
        };
        FileDownloadRequest: {
            path: string;
            range?: {
                start: number;
                endExclusive: number;
            };
        };
        InvitationCreateRequest: {
            email: string;
            /** @enum {string} */
            role: "admin" | "member";
        };
        LiveFileDownloadRequest: {
            path: string;
            range?: {
                start: number;
                endExclusive: number;
            };
            /** @enum {string} */
            wake?: "retained" | "never";
            /** @enum {string} */
            consistency?: "coherent" | "best_effort";
            ifGenerationId?: string;
        };
        LiveFileListRequest: {
            path?: string;
            recursive?: boolean;
            limit?: number;
            cursor?: string;
            /** @enum {string} */
            wake?: "retained" | "never";
            /** @enum {string} */
            consistency?: "coherent" | "best_effort";
            ifGenerationId?: string;
        };
        LiveFileStatRequest: {
            path: string;
            /** @enum {string} */
            wake?: "retained" | "never";
            /** @enum {string} */
            consistency?: "coherent" | "best_effort";
            ifGenerationId?: string;
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
        MetricAggregationRequest: {
            name: string;
            timeRange: {
                gte: string;
                lt: string;
            };
            interval: string;
            groupBy?: string[];
            calculations: ({
                /** @enum {string} */
                op: "count" | "sum" | "min" | "max" | "mean" | "increase" | "rate";
            } | {
                /** @constant */
                op: "quantile";
                q: number;
            })[];
            where?: unknown;
        };
        ObservationListenRequest: {
            signals?: ("events" | "logs" | "spans" | "metrics" | "traces")[];
            where?: unknown;
            timeRange?: {
                gte?: string;
                lt?: string;
            };
            order?: {
                /** @enum {string} */
                by: "time" | "accepted";
                /** @enum {string} */
                direction: "asc" | "desc";
            };
            limit?: number;
            consistency?: "available" | {
                /** @constant */
                mode: "caught_up";
                waitMs?: number;
            };
        };
        ObservationQuery: {
            signals?: ("events" | "logs" | "spans" | "metrics" | "traces")[];
            where?: unknown;
            timeRange?: {
                gte?: string;
                lt?: string;
            };
            order?: {
                /** @enum {string} */
                by: "time" | "accepted";
                /** @enum {string} */
                direction: "asc" | "desc";
            };
            limit?: number;
            consistency?: "available" | {
                /** @constant */
                mode: "caught_up";
                waitMs?: number;
            };
            cursor?: string;
        };
        ObservationStreamRequest: {
            signals?: ("events" | "logs" | "spans" | "metrics" | "traces")[];
            where?: unknown;
            timeRange?: {
                gte?: string;
                lt?: string;
            };
            order?: {
                /** @enum {string} */
                by: "time" | "accepted";
                /** @enum {string} */
                direction: "asc" | "desc";
            };
            limit?: number;
            consistency?: "available" | {
                /** @constant */
                mode: "caught_up";
                waitMs?: number;
            };
            origin: {
                cursor: string;
            } | {
                time: string;
            } | {
                /** @constant */
                earliest: true;
            };
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
        OrganizationCreateRequest: {
            name: string;
        };
        PersistedFileListRequest: {
            path?: string;
            recursive?: boolean;
            limit?: number;
            cursor?: string;
        };
        PersistedFileStatRequest: {
            path: string;
        };
        PortalSessionRequest: {
            returnUrl: string;
        };
        RegisteredFileDownloadRequest: {
            range?: {
                start: number;
                endExclusive: number;
            };
        };
        RegisteredFileInput: {
            mountPath: string;
            content: {
                /** @constant */
                type: "inline";
                /** @enum {string} */
                encoding: "utf8" | "base64";
                data: string;
                sha256: string;
            } | {
                /** @constant */
                type: "upload";
                uploadId: string;
                sha256: string;
                sizeBytes: number;
            };
            mediaType: string;
            /** @enum {string} */
            mode: "0644" | "0755";
        };
        RegisteredFileValue: {
            mountPath: string;
            content: components["schemas"]["BlobDescriptor"];
            mediaType: string;
            /** @enum {string} */
            mode: "0644" | "0755";
        };
        RegisteredInstructionInput: {
            text: string;
        };
        RegisteredInstructionValue: {
            text: string;
        };
        RegisteredMcpServerInput: {
            url: string;
            /** @constant */
            transport: "streamable_http";
            headers: {
                name: string;
                secretName: string;
            }[];
        };
        RegisteredMcpServerValue: {
            url: string;
            /** @constant */
            transport: "streamable_http";
            headers: {
                name: string;
                secretName: string;
            }[];
        };
        RegisteredResource: {
            /** @constant */
            kind: "file";
            name: string;
            revision: number;
            /** @constant */
            state: "current";
            sha256: string;
            sizeBytes: number;
            createdAt: string;
            updatedAt: string;
            value: components["schemas"]["RegisteredFileValue"];
        } | {
            /** @constant */
            kind: "skill";
            name: string;
            revision: number;
            /** @constant */
            state: "current";
            sha256: string;
            sizeBytes: number;
            createdAt: string;
            updatedAt: string;
            value: components["schemas"]["RegisteredSkillValue"];
        } | {
            /** @constant */
            kind: "tool";
            name: string;
            revision: number;
            /** @constant */
            state: "current";
            sha256: string;
            sizeBytes: number;
            createdAt: string;
            updatedAt: string;
            value: components["schemas"]["RegisteredToolValue"];
        } | {
            /** @constant */
            kind: "instruction";
            name: string;
            revision: number;
            /** @constant */
            state: "current";
            sha256: string;
            sizeBytes: number;
            createdAt: string;
            updatedAt: string;
            value: components["schemas"]["RegisteredInstructionValue"];
        } | {
            /** @constant */
            kind: "mcp_server";
            name: string;
            revision: number;
            /** @constant */
            state: "current";
            sha256: string;
            sizeBytes: number;
            createdAt: string;
            updatedAt: string;
            value: components["schemas"]["RegisteredMcpServerValue"];
        };
        RegisteredResourcePage: {
            items: ({
                /** @constant */
                kind: "file";
                name: string;
                revision: number;
                /** @constant */
                state: "current";
                sha256: string;
                sizeBytes: number;
                createdAt: string;
                updatedAt: string;
            } | {
                /** @constant */
                kind: "skill";
                name: string;
                revision: number;
                /** @constant */
                state: "current";
                sha256: string;
                sizeBytes: number;
                createdAt: string;
                updatedAt: string;
            } | {
                /** @constant */
                kind: "tool";
                name: string;
                revision: number;
                /** @constant */
                state: "current";
                sha256: string;
                sizeBytes: number;
                createdAt: string;
                updatedAt: string;
            } | {
                /** @constant */
                kind: "instruction";
                name: string;
                revision: number;
                /** @constant */
                state: "current";
                sha256: string;
                sizeBytes: number;
                createdAt: string;
                updatedAt: string;
            } | {
                /** @constant */
                kind: "mcp_server";
                name: string;
                revision: number;
                /** @constant */
                state: "current";
                sha256: string;
                sizeBytes: number;
                createdAt: string;
                updatedAt: string;
            })[];
            nextCursor?: string;
        };
        RegisteredSkillInput: {
            description: string;
            /** @constant */
            bundleFormat: "tar.gz";
            bundle: {
                /** @constant */
                type: "inline";
                /** @enum {string} */
                encoding: "utf8" | "base64";
                data: string;
                sha256: string;
            } | {
                /** @constant */
                type: "upload";
                uploadId: string;
                sha256: string;
                sizeBytes: number;
            };
        };
        RegisteredSkillValue: {
            description: string;
            /** @constant */
            bundleFormat: "tar.gz";
            bundle: components["schemas"]["BlobDescriptor"];
        };
        RegisteredToolInput: {
            description: string;
            inputSchema: {
                [key: string]: unknown;
            };
            entry: string;
            /** @constant */
            bundleFormat: "tar.gz";
            bundle: {
                /** @constant */
                type: "inline";
                /** @enum {string} */
                encoding: "utf8" | "base64";
                data: string;
                sha256: string;
            } | {
                /** @constant */
                type: "upload";
                uploadId: string;
                sha256: string;
                sizeBytes: number;
            };
        };
        RegisteredToolValue: {
            description: string;
            inputSchema: {
                [key: string]: unknown;
            };
            entry: string;
            /** @constant */
            bundleFormat: "tar.gz";
            bundle: components["schemas"]["BlobDescriptor"];
        };
        RegistryPutResult: {
            /** @enum {string} */
            status: "created" | "replaced" | "unchanged";
            resource: components["schemas"]["RegisteredResource"];
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
        SecretSetRequest: {
            value: string;
        };
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
                    /** @constant */
                    size: "512mb";
                    baseline: {
                        /** @constant */
                        memoryMiB: 512;
                        /** @constant */
                        vcpus: 0.25;
                    };
                    peak: {
                        /** @constant */
                        memoryMiB: 2048;
                        /** @constant */
                        vcpus: 1;
                    };
                    /** @constant */
                    maxDiskGiB: 8;
                    /** @constant */
                    endpointBandwidthMBps: 1;
                    /** @constant */
                    maxConcurrentConnections: 8;
                } | {
                    /** @constant */
                    size: "1gb";
                    baseline: {
                        /** @constant */
                        memoryMiB: 1024;
                        /** @constant */
                        vcpus: 0.5;
                    };
                    peak: {
                        /** @constant */
                        memoryMiB: 4096;
                        /** @constant */
                        vcpus: 2;
                    };
                    /** @constant */
                    maxDiskGiB: 8;
                    /** @constant */
                    endpointBandwidthMBps: 2;
                    /** @constant */
                    maxConcurrentConnections: 16;
                } | {
                    /** @constant */
                    size: "2gb";
                    baseline: {
                        /** @constant */
                        memoryMiB: 2048;
                        /** @constant */
                        vcpus: 1;
                    };
                    peak: {
                        /** @constant */
                        memoryMiB: 8192;
                        /** @constant */
                        vcpus: 4;
                    };
                    /** @constant */
                    maxDiskGiB: 8;
                    /** @constant */
                    endpointBandwidthMBps: 4;
                    /** @constant */
                    maxConcurrentConnections: 32;
                } | {
                    /** @constant */
                    size: "4gb";
                    baseline: {
                        /** @constant */
                        memoryMiB: 4096;
                        /** @constant */
                        vcpus: 2;
                    };
                    peak: {
                        /** @constant */
                        memoryMiB: 16384;
                        /** @constant */
                        vcpus: 8;
                    };
                    /** @constant */
                    maxDiskGiB: 16;
                    /** @constant */
                    endpointBandwidthMBps: 8;
                    /** @constant */
                    maxConcurrentConnections: 64;
                } | {
                    /** @constant */
                    size: "8gb";
                    baseline: {
                        /** @constant */
                        memoryMiB: 8192;
                        /** @constant */
                        vcpus: 4;
                    };
                    peak: {
                        /** @constant */
                        memoryMiB: 32768;
                        /** @constant */
                        vcpus: 16;
                    };
                    /** @constant */
                    maxDiskGiB: 32;
                    /** @constant */
                    endpointBandwidthMBps: 16;
                    /** @constant */
                    maxConcurrentConnections: 128;
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
                size?: "512mb" | "1gb" | "2gb" | "4gb" | "8gb";
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
        TelemetryExportRequest: {
            query: components["schemas"]["ObservationQuery"];
            /** @enum {string} */
            format: "ndjson" | "parquet" | "otlp_json";
            /** @enum {string} */
            completeness: "require" | "allow_gaps";
        };
        TelemetryGapQuery: {
            signals?: ("events" | "logs" | "spans" | "metrics" | "traces")[];
            /** @enum {string} */
            status?: "pending_retry" | "open" | "repaired";
            timeRange?: {
                gte?: string;
                lt?: string;
            };
            cursor?: string;
            limit?: number;
        };
        TopUpCheckoutRequest: {
            amountUsd: number;
            successUrl: string;
            cancelUrl: string;
        };
        Upload: {
            id: string;
            /** @enum {string} */
            state: "pending" | "uploading" | "ready" | "aborted";
            sizeBytes: number;
            sha256: string;
            contentType: string;
            createdAt: string;
            expiresAt: string;
        };
        UploadCompleteRequest: {
            parts: {
                partNumber: number;
                etag: string;
                sizeBytes: number;
                sha256: string;
            }[];
        };
        UploadCreateRequest: {
            sizeBytes: number;
            sha256: string;
            contentType: string;
        };
        UploadPartsRequest: {
            parts: {
                partNumber: number;
                sizeBytes: number;
                sha256: string;
            }[];
        };
        UploadPartsResponse: {
            parts: {
                partNumber: number;
                url: string;
                headers?: {
                    [key: string]: string;
                };
                expiresAt: string;
            }[];
        };
        UsageQuery: {
            categories?: ("storage" | "compute" | "data_transfer")[];
            timeRange: {
                gte: string;
                lt: string;
            };
            groupBy?: ("category" | "region" | "workspace" | "session" | "run" | "operation")[];
            cursor?: string;
            limit?: number;
        };
        WorkspaceApiKeyValue: string;
        WorkspaceCreateRequest: {
            organizationId: string;
            name: string;
            /** @enum {string} */
            region: "us-east-1" | "us-east-2" | "us-west-2" | "ap-northeast-1" | "eu-west-1";
        };
        WorkspaceDeleteRequest: {
            confirmation: string;
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
    "account.get": {
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
    "apiKeys.list": {
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
    "apiKeys.create": {
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
                "application/json": components["schemas"]["ApiKeyCreateRequest"];
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
    "apiKeys.delete": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                apiKeyId: string;
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
    "billing.balance": {
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
    "organizations.list": {
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
    "organizations.create": {
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
                "application/json": components["schemas"]["OrganizationCreateRequest"];
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
    "organizations.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                organizationId: string;
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
    "billing.autoTopup.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                organizationId: string;
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
    "billing.autoTopup.put": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                organizationId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["AutoTopupPolicyRequest"];
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
    "billing.portalSession": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                organizationId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["PortalSessionRequest"];
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
    "billing.statements.list": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                organizationId: string;
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
    "billing.statements.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                organizationId: string;
                statementId: string;
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
    "billing.statements.download": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                organizationId: string;
                statementId: string;
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
    "billing.topUpCheckout": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                organizationId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TopUpCheckoutRequest"];
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
    "invitations.create": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                organizationId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["InvitationCreateRequest"];
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
    "memberships.list": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                organizationId: string;
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
    "workspaces.list": {
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
    "workspaces.create": {
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
                "application/json": components["schemas"]["WorkspaceCreateRequest"];
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
    "workspaces.get": {
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "workspaces.delete": {
        parameters: {
            query?: never;
            header: {
                "Aex-Operation-Id": string;
            };
            path: {
                workspaceId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["WorkspaceDeleteRequest"];
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
}
