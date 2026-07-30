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
    "/api/billing/usage/query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** billing.usage.query */
        post: operations["billing.usage.query"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/events/listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** events.listen */
        post: operations["events.listen"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/events/query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** events.query */
        post: operations["events.query"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/events/stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** events.stream */
        post: operations["events.stream"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/logs/listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** logs.listen */
        post: operations["logs.listen"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/logs/query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** logs.query */
        post: operations["logs.query"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/logs/stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** logs.stream */
        post: operations["logs.stream"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/metrics/aggregate": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** metrics.aggregate */
        post: operations["metrics.aggregate"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/metrics/listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** metrics.listen */
        post: operations["metrics.listen"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/metrics/query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** metrics.query */
        post: operations["metrics.query"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/metrics/stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** metrics.stream */
        post: operations["metrics.stream"];
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
    "/api/sessions/{sessionId}/approvals": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** approvals.list */
        get: operations["approvals.list"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/approvals/{approvalId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** approvals.get */
        get: operations["approvals.get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/approvals/{approvalId}/responses": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** approvals.respond */
        post: operations["approvals.respond"];
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
    "/api/sessions/{sessionId}/events/listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.events.listen */
        post: operations["session.events.listen"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/events/query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.events.query */
        post: operations["session.events.query"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/events/stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.events.stream */
        post: operations["session.events.stream"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/files/live/downloads": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** files.live.download */
        post: operations["files.live.download"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/files/live/list": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** files.live.list */
        post: operations["files.live.list"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/files/live/stat": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** files.live.stat */
        post: operations["files.live.stat"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/files/persisted/downloads": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** files.persisted.download */
        post: operations["files.persisted.download"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/files/persisted/list": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** files.persisted.list */
        post: operations["files.persisted.list"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/files/persisted/stat": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** files.persisted.stat */
        post: operations["files.persisted.stat"];
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
    "/api/sessions/{sessionId}/logs/listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.logs.listen */
        post: operations["session.logs.listen"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/logs/query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.logs.query */
        post: operations["session.logs.query"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/logs/stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.logs.stream */
        post: operations["session.logs.stream"];
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
    "/api/sessions/{sessionId}/metrics/aggregate": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.metrics.aggregate */
        post: operations["session.metrics.aggregate"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/metrics/listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.metrics.listen */
        post: operations["session.metrics.listen"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/metrics/query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.metrics.query */
        post: operations["session.metrics.query"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/metrics/stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.metrics.stream */
        post: operations["session.metrics.stream"];
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
    "/api/sessions/{sessionId}/spans/listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.spans.listen */
        post: operations["session.spans.listen"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/spans/query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.spans.query */
        post: operations["session.spans.query"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/spans/stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.spans.stream */
        post: operations["session.spans.stream"];
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
    "/api/sessions/{sessionId}/telemetry/exports": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.telemetry.exports.create */
        post: operations["session.telemetry.exports.create"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/telemetry/exports/{exportId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** session.telemetry.exports.get */
        get: operations["session.telemetry.exports.get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/telemetry/exports/{exportId}/downloads": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.telemetry.exports.download */
        post: operations["session.telemetry.exports.download"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/telemetry/exports/{exportId}/revocations": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.telemetry.exports.revoke */
        post: operations["session.telemetry.exports.revoke"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/telemetry/gaps/{gapId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** session.telemetry.gaps.get */
        get: operations["session.telemetry.gaps.get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/telemetry/gaps/query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.telemetry.gaps.query */
        post: operations["session.telemetry.gaps.query"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/telemetry/listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.telemetry.listen */
        post: operations["session.telemetry.listen"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/telemetry/query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.telemetry.query */
        post: operations["session.telemetry.query"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/telemetry/stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.telemetry.stream */
        post: operations["session.telemetry.stream"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/traces/{traceId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** session.traces.get */
        get: operations["session.traces.get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/traces/listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.traces.listen */
        post: operations["session.traces.listen"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/traces/query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.traces.query */
        post: operations["session.traces.query"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/sessions/{sessionId}/traces/stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** session.traces.stream */
        post: operations["session.traces.stream"];
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
    "/api/spans/listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** spans.listen */
        post: operations["spans.listen"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/spans/query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** spans.query */
        post: operations["spans.query"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/spans/stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** spans.stream */
        post: operations["spans.stream"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/telemetry/exports": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** telemetry.exports.create */
        post: operations["telemetry.exports.create"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/telemetry/exports/{exportId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** telemetry.exports.get */
        get: operations["telemetry.exports.get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/telemetry/exports/{exportId}/downloads": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** telemetry.exports.download */
        post: operations["telemetry.exports.download"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/telemetry/exports/{exportId}/revocations": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** telemetry.exports.revoke */
        post: operations["telemetry.exports.revoke"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/telemetry/gaps/{gapId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** telemetry.gaps.get */
        get: operations["telemetry.gaps.get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/telemetry/gaps/query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** telemetry.gaps.query */
        post: operations["telemetry.gaps.query"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/telemetry/listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** telemetry.listen */
        post: operations["telemetry.listen"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/telemetry/otlp/v1/logs": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** telemetry.otlp.logs */
        post: operations["telemetry.otlp.logs"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/telemetry/otlp/v1/metrics": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** telemetry.otlp.metrics */
        post: operations["telemetry.otlp.metrics"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/telemetry/otlp/v1/traces": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** telemetry.otlp.traces */
        post: operations["telemetry.otlp.traces"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/telemetry/query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** telemetry.query */
        post: operations["telemetry.query"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/telemetry/stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** telemetry.stream */
        post: operations["telemetry.stream"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/traces/listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** traces.listen */
        post: operations["traces.listen"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/traces/query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** traces.query */
        post: operations["traces.query"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/traces/stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** traces.stream */
        post: operations["traces.stream"];
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
    "/api/workspace/files": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * registry.files.list
         * @description Current-view name-keyset pagination. This registry list is not a snapshot: deletes disappear, replacements may be observed, and names inserted at or before the cursor boundary may be missed. The opaque cursor does not bind page limit.
         */
        get: operations["registry.files.list"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/files/{name}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** registry.files.get */
        get: operations["registry.files.get"];
        /** registry.files.put */
        put: operations["registry.files.put"];
        post?: never;
        /** registry.files.delete */
        delete: operations["registry.files.delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/files/{name}/downloads": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** registry.files.download */
        post: operations["registry.files.download"];
        delete?: never;
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
        /**
         * registry.instructions.list
         * @description Current-view name-keyset pagination. This registry list is not a snapshot: deletes disappear, replacements may be observed, and names inserted at or before the cursor boundary may be missed. The opaque cursor does not bind page limit.
         */
        get: operations["registry.instructions.list"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/instructions/{name}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** registry.instructions.get */
        get: operations["registry.instructions.get"];
        /** registry.instructions.put */
        put: operations["registry.instructions.put"];
        post?: never;
        /** registry.instructions.delete */
        delete: operations["registry.instructions.delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/limits": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** workspace.limits.list */
        get: operations["workspace.limits.list"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/limits/{limitId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** workspace.limits.get */
        get: operations["workspace.limits.get"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/mcp-servers": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /**
         * registry.mcpServers.list
         * @description Current-view name-keyset pagination. This registry list is not a snapshot: deletes disappear, replacements may be observed, and names inserted at or before the cursor boundary may be missed. The opaque cursor does not bind page limit.
         */
        get: operations["registry.mcpServers.list"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/mcp-servers/{name}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** registry.mcpServers.get */
        get: operations["registry.mcpServers.get"];
        /** registry.mcpServers.put */
        put: operations["registry.mcpServers.put"];
        post?: never;
        /** registry.mcpServers.delete */
        delete: operations["registry.mcpServers.delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/secrets": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** secrets.list */
        get: operations["secrets.list"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/secrets/{secretId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** secrets.get */
        get: operations["secrets.get"];
        /** secrets.put */
        put: operations["secrets.put"];
        post?: never;
        /** secrets.delete */
        delete: operations["secrets.delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/secrets/{secretId}/revocations": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** secrets.revoke */
        post: operations["secrets.revoke"];
        delete?: never;
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
        /**
         * registry.skills.list
         * @description Current-view name-keyset pagination. This registry list is not a snapshot: deletes disappear, replacements may be observed, and names inserted at or before the cursor boundary may be missed. The opaque cursor does not bind page limit.
         */
        get: operations["registry.skills.list"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/skills/{name}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** registry.skills.get */
        get: operations["registry.skills.get"];
        /** registry.skills.put */
        put: operations["registry.skills.put"];
        post?: never;
        /** registry.skills.delete */
        delete: operations["registry.skills.delete"];
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
        /**
         * registry.tools.list
         * @description Current-view name-keyset pagination. This registry list is not a snapshot: deletes disappear, replacements may be observed, and names inserted at or before the cursor boundary may be missed. The opaque cursor does not bind page limit.
         */
        get: operations["registry.tools.list"];
        put?: never;
        post?: never;
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/tools/{name}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        /** registry.tools.get */
        get: operations["registry.tools.get"];
        /** registry.tools.put */
        put: operations["registry.tools.put"];
        post?: never;
        /** registry.tools.delete */
        delete: operations["registry.tools.delete"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/uploads": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** uploads.create */
        post: operations["uploads.create"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/uploads/{uploadId}": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        post?: never;
        /** uploads.abort */
        delete: operations["uploads.abort"];
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/uploads/{uploadId}/completion": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** uploads.complete */
        post: operations["uploads.complete"];
        delete?: never;
        options?: never;
        head?: never;
        patch?: never;
        trace?: never;
    };
    "/api/workspace/uploads/{uploadId}/parts": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        get?: never;
        put?: never;
        /** uploads.parts */
        post: operations["uploads.parts"];
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
    "billing.usage.query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["UsageQuery"];
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
    "events.listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationListenRequest"];
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
    "events.query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationQuery"];
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
    "events.stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationStreamRequest"];
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
    "logs.listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationListenRequest"];
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
    "logs.query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationQuery"];
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
    "logs.stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationStreamRequest"];
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
    "metrics.aggregate": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["MetricAggregationRequest"];
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
    "metrics.listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationListenRequest"];
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
    "metrics.query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationQuery"];
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
    "metrics.stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationStreamRequest"];
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
    "approvals.list": {
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
    "approvals.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
                approvalId: string;
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
    "approvals.respond": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
                approvalId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ApprovalResponseRequest"];
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
    "session.events.listen": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationListenRequest"];
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
    "session.events.query": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationQuery"];
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
    "session.events.stream": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationStreamRequest"];
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
    "files.live.download": {
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
                "application/json": components["schemas"]["LiveFileDownloadRequest"];
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
    "files.live.list": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["LiveFileListRequest"];
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
    "files.live.stat": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["LiveFileStatRequest"];
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
    "files.persisted.download": {
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
                "application/json": components["schemas"]["FileDownloadRequest"];
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
    "files.persisted.list": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["PersistedFileListRequest"];
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
    "files.persisted.stat": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["PersistedFileStatRequest"];
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
    "session.logs.listen": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationListenRequest"];
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
    "session.logs.query": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationQuery"];
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
    "session.logs.stream": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationStreamRequest"];
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
    "session.metrics.aggregate": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["MetricAggregationRequest"];
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
    "session.metrics.listen": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationListenRequest"];
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
    "session.metrics.query": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationQuery"];
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
    "session.metrics.stream": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationStreamRequest"];
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
    "session.spans.listen": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationListenRequest"];
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
    "session.spans.query": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationQuery"];
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
    "session.spans.stream": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationStreamRequest"];
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
    "session.telemetry.exports.create": {
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
        requestBody: {
            content: {
                "application/json": components["schemas"]["TelemetryExportRequest"];
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
    "session.telemetry.exports.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
                exportId: string;
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
    "session.telemetry.exports.download": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                sessionId: string;
                exportId: string;
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
    "session.telemetry.exports.revoke": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                sessionId: string;
                exportId: string;
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
    "session.telemetry.gaps.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
                gapId: string;
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
    "session.telemetry.gaps.query": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TelemetryGapQuery"];
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
    "session.telemetry.listen": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationListenRequest"];
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
    "session.telemetry.query": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationQuery"];
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
    "session.telemetry.stream": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationStreamRequest"];
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
    "session.traces.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
                traceId: string;
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
    "session.traces.listen": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationListenRequest"];
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
    "session.traces.query": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationQuery"];
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
    "session.traces.stream": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                sessionId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationStreamRequest"];
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
    "spans.listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationListenRequest"];
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
    "spans.query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationQuery"];
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
    "spans.stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationStreamRequest"];
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
    "telemetry.exports.create": {
        parameters: {
            query?: never;
            header: {
                "Aex-Operation-Id": string;
            };
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TelemetryExportRequest"];
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
    "telemetry.exports.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                exportId: string;
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
    "telemetry.exports.download": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                exportId: string;
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
    "telemetry.exports.revoke": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                exportId: string;
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
    "telemetry.gaps.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                gapId: string;
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
    "telemetry.gaps.query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["TelemetryGapQuery"];
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
    "telemetry.listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationListenRequest"];
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
    "telemetry.otlp.logs": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
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
    "telemetry.otlp.metrics": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
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
    "telemetry.otlp.traces": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
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
    "telemetry.query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationQuery"];
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
    "telemetry.stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationStreamRequest"];
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
    "traces.listen": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationListenRequest"];
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
    "traces.query": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationQuery"];
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
    "traces.stream": {
        parameters: {
            query?: never;
            header?: never;
            path?: never;
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["ObservationStreamRequest"];
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
    "registry.files.list": {
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
                content: {
                    "application/json": components["schemas"]["RegisteredResourcePage"];
                };
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
    "registry.files.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                name: string;
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
                content: {
                    "application/json": components["schemas"]["RegisteredResource"];
                };
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
    "registry.files.put": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                name: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["RegisteredFileInput"];
            };
        };
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RegistryPutResult"];
                };
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
    "registry.files.delete": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                name: string;
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
    "registry.files.download": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                name: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["RegisteredFileDownloadRequest"];
            };
        };
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["DownloadGrant"];
                };
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
    "registry.instructions.list": {
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
                content: {
                    "application/json": components["schemas"]["RegisteredResourcePage"];
                };
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
    "registry.instructions.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                name: string;
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
                content: {
                    "application/json": components["schemas"]["RegisteredResource"];
                };
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
    "registry.instructions.put": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                name: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["RegisteredInstructionInput"];
            };
        };
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RegistryPutResult"];
                };
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
    "registry.instructions.delete": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                name: string;
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
    "workspace.limits.list": {
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
                content: {
                    "application/json": components["schemas"]["EffectiveWorkspaceLimitPage"];
                };
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
    "workspace.limits.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                limitId: string;
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
                content: {
                    "application/json": components["schemas"]["EffectiveWorkspaceLimit"];
                };
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
    "registry.mcpServers.list": {
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
                content: {
                    "application/json": components["schemas"]["RegisteredResourcePage"];
                };
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
    "registry.mcpServers.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                name: string;
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
                content: {
                    "application/json": components["schemas"]["RegisteredResource"];
                };
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
    "registry.mcpServers.put": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                name: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["RegisteredMcpServerInput"];
            };
        };
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RegistryPutResult"];
                };
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
    "registry.mcpServers.delete": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                name: string;
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
                    "application/json": components["schemas"]["ApiError"];
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "secrets.put": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                secretId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["SecretSetRequest"];
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "secrets.revoke": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
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
                    "application/json": components["schemas"]["ApiError"];
                };
            };
        };
    };
    "registry.skills.list": {
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
                content: {
                    "application/json": components["schemas"]["RegisteredResourcePage"];
                };
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
    "registry.skills.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                name: string;
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
                content: {
                    "application/json": components["schemas"]["RegisteredResource"];
                };
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
    "registry.skills.put": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                name: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["RegisteredSkillInput"];
            };
        };
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RegistryPutResult"];
                };
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
    "registry.skills.delete": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                name: string;
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
    "registry.tools.list": {
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
                content: {
                    "application/json": components["schemas"]["RegisteredResourcePage"];
                };
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
    "registry.tools.get": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                name: string;
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
                content: {
                    "application/json": components["schemas"]["RegisteredResource"];
                };
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
    "registry.tools.put": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                name: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["RegisteredToolInput"];
            };
        };
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["RegistryPutResult"];
                };
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
    "registry.tools.delete": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                name: string;
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
    "uploads.create": {
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
                "application/json": components["schemas"]["UploadCreateRequest"];
            };
        };
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Upload"];
                };
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
    "uploads.abort": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                uploadId: string;
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
    "uploads.complete": {
        parameters: {
            query?: never;
            header: {
                "Idempotency-Key": string;
            };
            path: {
                uploadId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["UploadCompleteRequest"];
            };
        };
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["Upload"];
                };
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
    "uploads.parts": {
        parameters: {
            query?: never;
            header?: never;
            path: {
                uploadId: string;
            };
            cookie?: never;
        };
        requestBody: {
            content: {
                "application/json": components["schemas"]["UploadPartsRequest"];
            };
        };
        responses: {
            /** @description Success. */
            "2XX": {
                headers: {
                    [name: string]: unknown;
                };
                content: {
                    "application/json": components["schemas"]["UploadPartsResponse"];
                };
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
