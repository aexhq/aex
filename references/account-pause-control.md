---
title: Account pause admission and interruption control
description: Regional consistency model for stopping spend after an account pause without putting account state in an application cache.
keywords:
  - account pause
  - admission control
  - regional consistency
  - interruption
audience: maintainers and implementation agents
status: accepted
related:
  - references/architecture.md
  - references/rewrite/central-finance.md
  - references/rewrite/regional-services.md
---

# Account pause admission and interruption control

Account pause is a regional control event, not a session lifecycle state. The
regional workspace placement is the sole hot admission authority. Session
creation, message admission, and resume read that row on the request path and
repeat the active-account predicate inside their DynamoDB transaction. There is
no process-local or Redis account cache to invalidate.

The model deliberately permits bounded overdraft. Finance records usage first,
then publishes a monotone account epoch when the spendable balance is exhausted.
Regional propagation is eventual and interruption is best effort. Once the
paused placement commits, however, the following transaction conflict closes
the race with new work:

- Message admission writes an active-session locator in the same transaction as
  the run, session head, root admission, and account-active condition.
- Pause publication changes the placement row before enumerating locators.
- DynamoDB serializes the placement write against admission's condition check.
  Admission either commits first, in which case its locator is visible to the
  pause scan, or pause commits first, in which case admission is rejected.

Active locators are stored in the session authority under a workspace
partition, so the coordinator uses a strongly consistent base-table query and
does not depend on an eventually consistent secondary index. Terminal run
commit removes the locator atomically. Termination and deletion remove it when
they close the run authority themselves.

The central transactional outbox is the durable delivery authority. A regional
control invocation processes a bounded locator page and checkpoints its native
continuation under `(workspace, account epoch)`. An incomplete page leaves the
central outbox row retryable. The direct Aurora wake is the fast path; the
one-minute control recovery schedule is the slow backstop after finite Lambda
asynchronous retries. Every per-session interruption transaction also
checks that the placement is still paused at the exact account epoch, so a
delayed pause cannot interrupt work admitted after a later restore.

Interruption advances the session and Brain cancellation epochs, leaves session
work admission `open`, and installs `stopReason=account_paused` on the root
control row. The account placement—not every session head—therefore controls
admission after both pause and restore; restore needs no session fanout. A
running activation observes the epoch change on its lease-renewal interval,
cancels its in-flight provider work, and a successor settles the run as
`interrupted(account_paused)`. The retained MicroVM and the session remain
reusable after the account is restored.

Pause-exempt cleanup operations still condition on the observed account epoch
and organization, but not on active status. Cost-incurring operations use the
strict active-account condition. This keeps cancellation, suspension,
termination, and deletion available during a pause without weakening the
transaction-time fence on create, message send, or resume.

Operationally, measure finance-to-placement and placement-to-root-stop latency
separately. Alert on old undispatched central outbox rows, incomplete regional
pause checkpoints, conditional-conflict retry exhaustion, and active locators
that outlive their run. The intended stop window is propagation latency plus one
Brain renewal interval; it is not a zero-overdraft guarantee.
