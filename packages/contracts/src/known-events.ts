import type { ProviderEvent } from "./runtime-types.js";

/**
 * Type guards that narrow a `ProviderEvent` by the documented Claude Managed
 * Agents event `type` strings. These do not assert anything about the event
 * payload shape beyond the `type` field — payload contents are passed through
 * verbatim from the provider and may evolve.
 *
 * Reference: https://platform.claude.com/docs/en/managed-agents/events-and-streaming.md
 */

// Prefix groupers — documented `{domain}.{action}` convention.

export function isAgentEvent<E extends ProviderEvent>(event: E): event is E & { type: `agent.${string}` } {
  return typeof event.type === "string" && event.type.startsWith("agent.");
}

export function isUserEvent<E extends ProviderEvent>(event: E): event is E & { type: `user.${string}` } {
  return typeof event.type === "string" && event.type.startsWith("user.");
}

export function isSessionEvent<E extends ProviderEvent>(event: E): event is E & { type: `session.${string}` } {
  return typeof event.type === "string" && event.type.startsWith("session.");
}

export function isSpanEvent<E extends ProviderEvent>(event: E): event is E & { type: `span.${string}` } {
  return typeof event.type === "string" && event.type.startsWith("span.");
}

// Agent events.

export function isAgentMessage<E extends ProviderEvent>(event: E): event is E & { type: "agent.message" } {
  return event.type === "agent.message";
}

export function isAgentToolUse<E extends ProviderEvent>(event: E): event is E & { type: "agent.tool_use" } {
  return event.type === "agent.tool_use";
}

export function isAgentToolResult<E extends ProviderEvent>(event: E): event is E & { type: "agent.tool_result" } {
  return event.type === "agent.tool_result";
}

export function isAgentThinking<E extends ProviderEvent>(event: E): event is E & { type: "agent.thinking" } {
  return event.type === "agent.thinking";
}

export function isAgentCustomToolUse<E extends ProviderEvent>(event: E): event is E & { type: "agent.custom_tool_use" } {
  return event.type === "agent.custom_tool_use";
}

export function isAgentMcpToolUse<E extends ProviderEvent>(event: E): event is E & { type: "agent.mcp_tool_use" } {
  return event.type === "agent.mcp_tool_use";
}

export function isAgentMcpToolResult<E extends ProviderEvent>(event: E): event is E & { type: "agent.mcp_tool_result" } {
  return event.type === "agent.mcp_tool_result";
}

// User events.

export function isUserMessage<E extends ProviderEvent>(event: E): event is E & { type: "user.message" } {
  return event.type === "user.message";
}

// Session events.

export function isSessionStatusIdle<E extends ProviderEvent>(event: E): event is E & { type: "session.status_idle" } {
  return event.type === "session.status_idle";
}

export function isSessionStatusRunning<E extends ProviderEvent>(event: E): event is E & { type: "session.status_running" } {
  return event.type === "session.status_running";
}

export function isSessionStatusRescheduled<E extends ProviderEvent>(event: E): event is E & { type: "session.status_rescheduled" } {
  return event.type === "session.status_rescheduled";
}

export function isSessionStatusTerminated<E extends ProviderEvent>(event: E): event is E & { type: "session.status_terminated" } {
  return event.type === "session.status_terminated";
}

export function isSessionError<E extends ProviderEvent>(event: E): event is E & { type: "session.error" } {
  return event.type === "session.error";
}
