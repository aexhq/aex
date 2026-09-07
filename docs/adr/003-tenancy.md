# ADR-003: Make account ownership durable at the Aex boundary

Status: Accepted. Date: 2026-09-07. Depends on ADR-002.

## Context

A deployment bearer, random session IDs and scoped Brain host tokens do not establish which
customer may create bindings or operate a resource. Brain also scopes create keys globally.
Aex needs product state even when it does not yet bill customers.

## Decision

One account is one tenant in MVP. An operator creates accounts and issues or revokes random
high-entropy API keys. Store only key verifiers, identifiers and status; reveal the key on
issuance. Use established randomness, hashing and comparison libraries. There are no
passwords, JWT issuer, organization hierarchy or role framework in this slice.

Resolve each account-key request to an explicit principal. Every operation names its
account-owned resource; a missing or differently owned resource has the same external
not-found behavior. Authorize before reading history or forwarding a mutation. Account
lists are queried within the account. Ownership is immutable in MVP.

Minimum product records are accounts, API-key verifiers, session ownership, host ownership,
and create-operation claims. Pending deletion state belongs with session ownership.
Store lifecycle status only when needed to coordinate an Aex operation; Brain remains the
source of runtime status. Do not copy transcripts, Events or model usage into a second journal.

Host registration requires an account key. Persist its owning account and issuing key before
returning the registration. Session creation accepts only hosts owned by that account.
Commands/results/emits preserve Brain's host-token validation and also consult Aex ownership
and current account/key status. Scoped host credentials are not general session access keys.
Key revocation closes streams opened under that key and disallows its host registrations;
affected applications re-register/recreate as necessary. It does not retroactively undo a
dispatched effect or silently cancel an accepted turn. Account suspension denies new work
and access; operator cancellation uses ordinary Brain session operations.

## Create and retry behavior

1. Validate the request and product policy. In one product transaction, reserve the account
   session allowance and claim `(account, operation, client key)` with its request fingerprint
   and a unique persisted upstream request key. Different requests using the same key conflict.
2. Send the allowed request to Brain once. Do not store plaintext model credentials in this
   claim. The caller supplies the same request, including credentials, for any explicit retry.
3. On a completed create response, atomically persist session ownership and the operation
   result. Only then return success to the customer.
4. A lost response is pending/ambiguous, never a new request key. The MVP never dispatches a pending Aex claim again.
   Surface uncertainty and retain the reservation for operator resolution. Only an Aex
   claim with a completed saved result can replay.

Brain's saved-response retention is not an infinite exactly-once guarantee. Once that
window has passed, do not resubmit an unresolved create: Brain could create a second session.
A claim left before dispatch is also uncertain after a crash; absence of a saved response
does not prove absence of execution. M0 must test these cases with the actual pinned version.

If Brain created a resource but Aex cannot associate its result, the resource remains
inaccessible. After quiescing creation and resolving in-flight gateway requests, operator
comparison of Brain's inventory with Aex ownership can identify orphans for cleanup; it
must not infer a tenant from request similarity or timing. Host
registration uses the same success-after-ownership rule; failed association can leave an
unused host for Brain's normal expiry, never a usable unowned registration.

For ordinary owned session mutations, scope keys consistently and rely on Brain's existing
claims. Avoid adding another general mutation replay engine. HTTP retry and effect retry
are distinct; the gateway never causes an uncertain model/Tool effect to run again.

Deletion durably marks the owned resource as deleting before forwarding cleanup. Reject new
work, retain enough ownership to complete the same operation, and return deletion success
only after confirmed Brain cleanup. A timeout is pending/unknown. Verify Brain's retry and
not-found behavior before treating absence as completion; do not drop ownership on timeout.

## Why these controls exist

| Failure in supported use | Response |
| --- | --- |
| Customer submits another account's session or host ID | Deny before access/binding |
| Two customers choose the same create key | Distinct durable upstream claims |
| Aex crashes after Brain create but before ownership commit | No leaked resource; explicit recovery/cleanup |
| Revoked key retains an open feed or host token | Close associated streams; deny subsequent scoped traffic |
| Duplicate or interrupted deletion | Preserve ownership/tombstone until cleanup is confirmed |

These are concrete customer-boundary failures, consistent with
[OWASP's object-authorization guidance](https://owasp.org/API-Security/editions/2023/en/0xa1-broken-object-level-authorization/).

## Consequences and acceptance

Manual intervention for rare unresolved creates is an explicit MVP limitation, subject to
owner acceptance. A general saga/outbox/lease framework would not remove the upstream
ambiguity and is not introduced. If that limitation is unacceptable, settle the neutral
Brain integration seam before M1.

Acceptance requires two-account tests for every supported route class, concurrent duplicate
create tests, failures around each durable boundary, revoked-stream tests, and restart tests.
No account identity is added to the Brain journal or its neutral protocol.
