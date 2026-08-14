"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { ROUTES, newId, type RouteId } from "@aexhq/sdk";

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
  /** Present for POST-shaped bounded reads such as usage queries. */
  readonly body?: unknown;
}

export interface Resource<T> {
  readonly state: PanelState<T>;
  readonly reload: () => void;
}

/**
 * The complete identity of one panel request, by value.
 *
 * A panel builds its `parameters` and `body` as fresh object literals on every
 * render, so referential identity is useless as an effect dependency — depending on
 * it would re-fire the request, set the loading state, re-render, and start again.
 * This value is the only dependency the request effect has, and it is equal for
 * equal inputs no matter how many objects were allocated to express them.
 */
export function requestKey(
  routeId: RouteId,
  options: ReadOptions,
  nonce: number,
): string {
  return JSON.stringify([
    routeId,
    options.region ?? null,
    Object.entries(options.parameters ?? {}).filter(([, value]) => value !== undefined).sort(),
    options.body ?? null,
    options.enabled ?? true,
    options.deadlineMs ?? DEADLINE_MS.control,
    nonce,
  ]);
}

/**
 * One panel, one request, one deadline, one abort. `useResource` covers both the
 * GET collection reads and the POST-shaped bounded queries; the descriptor decides
 * the method, so a panel names an operation and its parameters and nothing else.
 */
export function useResource<T>(routeId: RouteId, options: ReadOptions = {}): Resource<T> {
  const [state, setState] = useState<PanelState<T>>({ kind: "loading" });
  const [nonce, setNonce] = useState(0);
  const key = requestKey(routeId, options, nonce);
  const current = useRef({ key, routeId, options });
  current.current = { key, routeId, options };

  useEffect(() => {
    const { options: input } = current.current;
    if (input.enabled === false) return undefined;
    const descriptor = ROUTES[routeId];
    const deadlineMs = input.deadlineMs ?? DEADLINE_MS.control;
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
        const response = await fetch(panelUrl(routeId, input.parameters ?? {}, input.region), {
          method: descriptor.method,
          headers,
          credentials: "same-origin",
          ...(descriptor.method === "GET" ? {} : { body: JSON.stringify(input.body ?? {}) }),
          signal: controller.signal,
        });
        const next = await readResponse<T>(response);
        if (current.current.key === key) setState(next);
      } catch {
        if (current.current.key === key) {
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
    // `key` is the complete identity of this request, by value. It is deliberately
    // the ONLY dependency: a panel rebuilds its parameters and body as fresh
    // literals every render, so any referential dependency here would re-fire the
    // request forever. `test/client.test.ts` pins both halves of that invariant.
  }, [key, routeId]);

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
  if (descriptor.idempotency === "operation_id") headers["aex-operation-id"] = newId("operation");
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
