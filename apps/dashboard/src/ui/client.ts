"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { ROUTES, type RouteId } from "@aexhq/sdk";

import { classifyFailure, parseRetryAfter, type PanelState } from "./panel";

/**
 * Panel deadlines. Control surfaces must answer fast or say they cannot; analytics
 * are allowed to take longer and are never permitted to hold up anything else.
 * Every panel owns its own `AbortController`, so one slow query cancels alone and
 * unmounting a panel cancels its request rather than leaking it.
 */
export const DEADLINE_MS = {
  control: 1_500,
  resources: 2_000,
  analytics: 3_000,
} as const;

export type Deadline = (typeof DEADLINE_MS)[keyof typeof DEADLINE_MS];

export type Parameters = Readonly<Record<string, string | undefined>>;

export function panelUrl(routeId: RouteId, parameters: Parameters, region?: string): string {
  const descriptor = ROUTES[routeId];
  const search = new URLSearchParams();
  if (descriptor.plane === "regional") {
    if (!region) throw new Error(`${routeId} is a regional operation and needs a region`);
    search.set("region", region);
  }
  for (const [name, value] of Object.entries(parameters)) {
    if (value !== undefined) search.set(name, value);
  }
  const query = search.toString();
  return query ? `/api/v1/${descriptor.plane}/${routeId}?${query}` : `/api/v1/${descriptor.plane}/${routeId}`;
}

function csrfToken(): string {
  const match = /(?:^|;\s*)__Host-aex_csrf=([^;]*)/.exec(document.cookie);
  return match?.[1] ? decodeURIComponent(match[1]) : "";
}

async function readResponse<T>(response: Response): Promise<PanelState<T>> {
  const body: unknown = response.status === 204 ? null : await response.json().catch(() => null);
  if (response.ok) return { kind: "ready", data: body as T };
  return classifyFailure(response.status, body, parseRetryAfter(response.headers.get("retry-after")));
}

function unreachable(): PanelState<never> {
  return {
    kind: "unavailable",
    failure: { code: "upstream_error", message: "the request could not be completed", retryable: true },
    retryAfterMs: null,
  };
}

export interface ReadOptions {
  readonly region?: string;
  readonly parameters?: Parameters;
  readonly deadlineMs?: Deadline;
  /** A panel that is not yet answerable — no workspace chosen, no id — stays idle. */
  readonly enabled?: boolean;
  /** Present for the POST-shaped bounded reads: observation and usage queries. */
  readonly body?: unknown;
}

export interface Resource<T> {
  readonly state: PanelState<T>;
  readonly reload: () => void;
}

/**
 * One panel, one request, one deadline, one abort. `useResource` covers both the
 * GET collection reads and the POST-shaped bounded queries; the descriptor decides
 * the method, so a panel names an operation and its parameters and nothing else.
 */
export function useResource<T>(routeId: RouteId, options: ReadOptions = {}): Resource<T> {
  const { region, parameters, body, enabled = true } = options;
  const deadlineMs = options.deadlineMs ?? DEADLINE_MS.control;
  const [state, setState] = useState<PanelState<T>>({ kind: "loading" });
  const [nonce, setNonce] = useState(0);
  const key = JSON.stringify([routeId, region ?? null, parameters ?? {}, body ?? null, enabled, nonce]);
  const latest = useRef(key);

  useEffect(() => {
    latest.current = key;
    if (!enabled) return undefined;
    const descriptor = ROUTES[routeId];
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort("deadline"), deadlineMs);
    setState({ kind: "loading" });

    void (async () => {
      try {
        const headers: Record<string, string> = { accept: "application/json" };
        if (descriptor.method !== "GET") {
          headers["content-type"] = "application/json";
          headers["x-aex-csrf"] = csrfToken();
        }
        const response = await fetch(panelUrl(routeId, parameters ?? {}, region), {
          method: descriptor.method,
          headers,
          credentials: "same-origin",
          ...(descriptor.method === "GET" ? {} : { body: JSON.stringify(body ?? {}) }),
          signal: controller.signal,
        });
        const next = await readResponse<T>(response);
        if (latest.current === key) setState(next);
      } catch {
        if (latest.current === key) {
          setState(controller.signal.aborted ? { kind: "timeout", deadlineMs } : unreachable());
        }
      } finally {
        clearTimeout(timer);
      }
    })();

    return () => {
      clearTimeout(timer);
      controller.abort("superseded");
    };
    // `key` is the complete identity of this request; nothing else may re-fire it.
  }, [key, routeId, region, parameters, body, enabled, deadlineMs]);

  const reload = useCallback(() => setNonce((value) => value + 1), []);
  return { state, reload };
}

export interface MutationInput {
  readonly region?: string;
  readonly parameters?: Parameters;
  readonly body?: unknown;
}

/**
 * One user action, one wire call. An operation the contract marks
 * `idempotency_key` carries a key minted here, so an accidental double submit is
 * the same submit rather than a second one. A render never causes a call.
 */
export async function submit<T>(routeId: RouteId, input: MutationInput = {}): Promise<PanelState<T>> {
  const descriptor = ROUTES[routeId];
  const headers: Record<string, string> = { accept: "application/json", "x-aex-csrf": csrfToken() };
  if (descriptor.idempotency === "idempotency_key") headers["idempotency-key"] = crypto.randomUUID();
  if (input.body !== undefined) headers["content-type"] = "application/json";
  try {
    const response = await fetch(panelUrl(routeId, input.parameters ?? {}, input.region), {
      method: descriptor.method,
      headers,
      credentials: "same-origin",
      ...(input.body === undefined ? {} : { body: JSON.stringify(input.body) }),
    });
    return await readResponse<T>(response);
  } catch {
    return unreachable();
  }
}
