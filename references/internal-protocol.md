---
title: "the *.internal runtime protocol"
description: The six virtual hostnames the aex session runtime addresses, what each one is for, which credentials the managed boundary injects on the far side, and why publishing the topology discloses nothing the shipped bundles did not already contain.
keywords:
  - internal protocol
  - egress
  - virtual host
  - runtime boundary
  - credential injection
audience: contributors, security researchers, and implementation agents
status: accepted
related:
  - references/architecture.md
  - references/glossary.md
  - references/rules.md
---

# The `*.internal` runtime protocol

Published aex runtime code dials six hostnames that do not resolve on the public
internet:

```
egress.internal    web.internal    llm.internal
journal.internal   events.internal aex.internal
```

Plus one real intra-network name, the credential broker at
`http://egress.aex.internal:8787`.

If you have just cloned this repository and grepped for one of those, this page
is the answer. They are not missing services, not placeholder names, and not a
leaked internal inventory. They are the wire form of a trust boundary.

## Why the runtime talks to hosts that do not exist

A session runs customer-authored code and model-authored code in the same
container. That container is **untrusted** — it is the thing being sandboxed, so
it cannot hold a credential that outlives the request it is making, and it cannot
be trusted to decide which upstream it is allowed to reach.

The design consequence is that the runtime never opens an outbound connection
itself. It makes an ordinary HTTP request to a virtual host, and a managed
boundary outside the container terminates it. That boundary is where the real
decisions happen:

1. resolve which session the caller is (from container identity, never from a
   bearer the container holds);
2. validate the requested target against the deny-list and, where applicable, the
   session's declared allowlist;
3. inject the privileged credential the request needs;
4. forward, meter, and record.

So a virtual host is not an address. It is a **capability name**: "the thing I am
allowed to ask for", with the authority to actually do it held on the other side.

That is why the topology is safe to publish. A reader learns the shape of the
requests the runtime makes. Nothing about that shape is a credential, and the
runtime holding the shape is precisely the component the design assumes is
compromised.

## The disclosure already happened

These constants are compiled into the tool bundles that ship inside the runner
image, and have been for as long as those bundles have existed. Six of the eleven
committed tool bundles contain at least one `*.internal` host literal. The choice
was never "publish or not"; it was "publish documented or publish undocumented".

## The six hosts

Every hop below is plain `http://`. None of them leaves the machine — the request
is terminated by a boundary process on the same host — so there is no transport
to encrypt and no certificate to verify.

### `egress.internal` — customer egress

The runtime's outbound path for anything the **customer** declared: named proxy
endpoints and remote MCP servers.

The runtime resolves the real upstream URL, injects the customer's own
credentials from its RAM-only sealed blob, and dials `egress.internal` naming the
resolved URL in `x-aex-egress-target`. The boundary then validates that target
against **both** the SSRF deny-list **and** the session's declared allowlist
before forwarding. An empty allowlist denies everything.

Two independent gates, because the container is untrusted: it chose the target,
so it does not get to be the one that approves it.

### `web.internal` — platform web tools

The outbound path for the `web_fetch` and `web_search` builtins. Symmetric to
`egress.internal`, with one deliberate difference: it is **not** gated by the
session's declared allowlist, because `web_fetch` is arbitrary-URL by design. The
SSRF deny-list plus a resolve-then-check DNS-rebind guard is the only gate.

It has two modes, selected by a marker header:

| Mode | Target | Credential injected |
| --- | --- | --- |
| `web_fetch` | Arbitrary customer URL in `x-aex-egress-target` | None |
| `web_search` | Must be the fixed search host | The platform search key, added by the boundary |

The search key never enters the container. The boundary verifies the target is
the exact expected search host before injecting it, so a `web_fetch` — or a
malicious container — cannot steer the key to an upstream of its choosing.

### `llm.internal` — model calls

The runtime's path to the model gateway. Under managed model access there is
exactly one gateway and one platform-owned key per plane, so this host has no
per-provider fan-out. The container never holds the gateway key.

### `journal.internal` — object store

The runtime's path to the session object store: journal entries, captured files,
checkpoints, and archives. Keys are built by a single owned key-builder module
rather than composed at call sites, so the namespace a request can name is a
closed set.

### `events.internal` — the event stream

The brain's path for forwarding its projected customer event stream to the events
coordinator. The brain holds no coordinator credential; it POSTs the coordinator
batch shape to `/sessions/<sessionId>/events` and the boundary injects the
privileged coordinator bearer before forwarding. Method and path are preserved
verbatim — only the secret is added.

### `aex.internal` — the session vault key

`GET http://aex.internal/session-vault-key` returns the session's
self-contained-secrets decryption key. The boundary resolves the session from the
container's identity — there is no bearer to steal — and returns either the key
or a typed error.

An in-process subagent shares its parent's container, so container identity
resolves to the *parent*. A child names its own session id in a query parameter,
and the boundary serves the child's key only after confirming from the session
record that it really is a child of that parent.

## The credential broker

`http://egress.aex.internal:8787/session-creds` is a real intra-network address,
not a virtual host. It issues short-lived session-scoped credentials to the
runtime, authenticated by `x-aex-writer-token` — a per-session capability, not an
account credential. A child session's fetch is scoped to that child.

## Header contract

These exact lowercase byte sequences cross the runtime boundary. They are part of
the protocol, so they are frozen; a parity test in the private repository keeps
the boundary implementation's copy identical.

| Header | Carried by | Meaning |
| --- | --- | --- |
| `x-aex-egress-target` | Runtime → boundary | The real upstream URL this virtual-host hop is for. |
| `x-aex-writer-token` | Runtime → control plane | Per-session writer capability. |
| `x-aex-egress-token` | Runtime → boundary | Per-session data-egress authorization and billing capability. |
| `x-aex-web-tool-capability` | Trusted runtime → `web.internal` | Authorizes the managed web-tool hop. Customer subprocesses must never receive or forward it. |
| `x-aex-brain-token` | Brain → control-plane hosts | Per-session token attached to every `llm.internal`, `aex.internal`, `journal.internal`, and `events.internal` dial. The boundary refuses a control-plane request without it. |

The brain token is defence in depth, not the primary control: customer processes
run as unprivileged per-call UIDs and never receive it, and network policy plus
token audience checks remain independent layers.

## What is published and what is not

**Published, here:** the hostnames, the request shapes, the header names, the
paths, and the ordering of the checks — everything the runtime in this repository
needs to speak the protocol.

**Not published:** the boundary implementation itself. It is the sole internet
path for untrusted customer code, and its value as a control does not come from
obscurity — but its token format, MAC construction, and admission-check
implementation are part of the hosted plane, which is private for the same reason
the rest of the control plane is.

That asymmetry is deliberate and worth stating so nobody spends an afternoon
looking for the other half in this repository. It is not here.

## For security researchers

The property to attack is this: **the container is assumed hostile, and every
privileged credential is injected on the far side of the boundary.** A finding
that matters is one where a request the runtime can construct causes the boundary
to inject a credential for an upstream it should not have, or to reach a target
the deny-list should have refused.

A finding that does not matter is that the hostnames are guessable. They are
published on this page.

Report through [`SECURITY.md`](../SECURITY.md).
