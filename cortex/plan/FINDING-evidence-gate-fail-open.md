# Finding — the client-side evidence gate fails open, and is not the boundary

**Filed:** 2026-08-17 (wave 6 / task 3, during the repo foundation pass)
**Site:** `cortex/src/lib/taskManager.ts`, `validateTaskCompletion`
**Plan reference:** flagged as a Phase 15 launch blocker
**Status:** written up, deliberately not fixed in `chore/repo-foundation`

## What was asked

Fix it if it is a one-liner, otherwise write it up. It is not a one-liner. It is
two defects with opposite fixes, and one of them is a product decision.

## What the code does

```ts
async function validateTaskCompletion(groupId, taskId) {
  try {
    const evidenceCheck = await checkTaskEvidence(groupId, taskId);
    if (!evidenceCheck.completion_gate.gated_done) {
      return { canComplete: false, reason: `Evidence validation required: …` };
    }
    return { canComplete: true };
  } catch (error) {
    console.warn('Evidence validation failed, allowing completion:', error);
    return { canComplete: true };   // ← fail open
  }
}
```

Three call sites, all in the same file: `updateTask`, and the two action
handlers that move a task to `done`.

The happy path fails *closed* — an explicit `gated_done: false` blocks
completion. Only the `catch` fails open: a network error, a 500, a timeout, or a
route that is not there is read as permission to proceed.

## The correction that matters most: this is not the security boundary

**The server already enforces this, on every write path.**
`require_evidence_for_done_transitions` (`crates/api/src/integrations.rs:1320`)
refuses any transition to `done` without an evidence-backed verified run, and it
is called from all four routes that can write task state:

| Route | Handler | Line |
| --- | --- | --- |
| `POST /api/groups/{group_id}/tasks` | `create_group_task` | `integrations.rs:478` |
| `PUT /api/groups/{group_id}/tasks` | `update_group_tasks` | `integrations.rs:509` |
| `PATCH /api/groups/{group_id}/tasks/{task_id}` | `patch_group_task` | `integrations.rs:568` |
| `POST /api/groups/{group_id}/tasks/actions` | `apply_group_task_actions` | `integrations.rs:1053` |

It resolves through `Database::cortex_task_has_evidence_backed_completion`
(`db.rs:6472`) into `cortex_completion_gate` (`db.rs:4788`), which requires
`reason == "passed"`.

So a customer cannot mark an unverified task done by defeating the client. The
client-side check is a **UX pre-check**, and the fail-open is not an authorization
bypass. Anyone reading the Phase 15 blocker as "unverified work can be accepted"
should read it again against these call sites.

## What is actually wrong

### 1. Silent client/server divergence

On the fail-open branch the client proceeds: it writes the task as `done` into
its local state and into `localStorage`, publishes the activity entry, and then
sends the patch. The server refuses with `400`. The user is looking at a board
that says the task is done and a backend that says it is not, and the two only
reconcile on the next successful `loadRemoteState`.

That divergence is the real defect, and flipping the `catch` to
`canComplete: false` does not fix it — it only changes which side of the split
the user sees first.

### 2. Flipping the catch trades one failure for a worse one

`checkTaskEvidence` throws on any non-2xx or transport failure. Fail-closed
means every transient blip — a dropped connection, a restarting API, a slow
response — becomes "you may not complete this task", with a reason string that
says evidence validation failed when in fact nothing was validated at all.

There is no way to distinguish "the gate says no" from "the gate did not answer"
in the current return type, and telling a user their verified work is unverified
because a socket closed is a worse product failure than a UI that optimistically
advances and is corrected a second later.

### 3. The message is wrong either way

`Evidence validation required: …` and `Cannot mark task as done: evidence
validation failed` both assert a verdict. On the `catch` branch no verdict
exists.

## What a real fix looks like

Not done here; this is the shape it should take.

1. **Widen the return type** to three states rather than two — `allowed`,
   `refused(reason)`, `unknown(error)` — so the caller can tell a verdict from a
   missing verdict. This is the change that unblocks the other two.
2. **Let `unknown` proceed, and mark it.** Keep the optimistic write, but tag the
   local task as unconfirmed and let the server's `400` demote it, with a visible
   message that names the transport failure rather than claiming a verdict.
3. **Surface the server's refusal.** The `400` from
   `require_evidence_for_done_transitions` carries a usable message
   (`task {id} cannot be marked done until Cortex has a successful verified
   run`). Show that, instead of the client's guess at it.
4. **Test it at the boundary, not near it.** There is no test anywhere that
   asserts what the UI does when `checkTaskEvidence` throws. That absence is why
   this survived: the gate's own tests are server-side, where the gate is
   correct.

## Recommended disposition

Reclassify. The Phase 15 launch blocker should read as **"the completion gate's
client behaviour is wrong on the error path and diverges from the server"**, not
as "the completion gate can be bypassed". The server-side gate holds. The
severity is user-facing correctness and trust in the board, not authorization —
which also means it does not need to block a launch that the worker-credential
and provider-key gaps already block.
