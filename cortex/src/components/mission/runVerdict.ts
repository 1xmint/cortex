import type { RunStep } from '../../lib/cortexApi';

// A run's own status can read "completed" while a step underneath it was
// independently verified as `Failed` -- verification finishes after
// execution does. The backend is the real gate (POST .../pr answers 409 for
// exactly this, keyed on the latest sealed verdict, not on whether a refund
// landed), but showing an "Open pull request" button that is certain to be
// refused is its own kind of lie. Same rule the backend applies: any step's
// latest verdict reading `failed` withholds the whole run's PR.
//
// `status: 'failed'` on a step is not only a verification verdict, though:
// `fail_step` (crates/api/src/db/mod.rs) also lands a step on `failed`
// straight from `leased`/`running` when the worker's own execution failed,
// before verification ever started. Both are real failures and both belong
// here -- this only answers "did a step fail", not "was it a verification
// verdict" (see `hasFailedVerificationStep` for that distinction).
export function hasFailedStep(steps: RunStep[]): boolean {
  return steps.some((step) => step.status === 'failed');
}

// Whether any failed step's failure is specifically a verification verdict,
// as opposed to an execution failure that never reached verification.
// `deliver_attempt` (crates/api/src/db/mod.rs ~10578) sets the step's
// `latest_attempt.status` to `delivered`, and a verdict landing afterward
// never changes it -- only `fail_step` does, by calling `fail_attempt`
// (which sets `latest_attempt.status` to `failed`). So a step that is
// `status: 'failed'` with a `latest_attempt.status` of `delivered` failed
// after delivery, i.e. during verification; one whose `latest_attempt.status`
// is `failed` failed during execution, before delivery. Callers use this to
// pick copy: "failed verification" is only accurate for the former.
export function hasFailedVerificationStep(steps: RunStep[]): boolean {
  return steps.some(
    (step) => step.status === 'failed' && step.latest_attempt?.status === 'delivered',
  );
}

// A step can be mid-verification -- delivered but not yet graded -- in which
// case its verdict could still land on `Failed`. The backend refuses the PR
// for this too (409 `verification_pending`), so the button should not
// promise something that request would then refuse.
//
// Mirrors the backend's `run_has_pending_verification` (crates/api/src/db/
// ledger.rs): only a step whose verification is actually in flight counts.
// A `recovered` step (the original of a heal, flipped by `mark_step_recovered`
// once its retry exists) is not verifying any more -- the retry is -- so it
// must not hide the button or read as pending.
export function hasPendingVerification(steps: RunStep[]): boolean {
  return steps.some((step) => step.status === 'delivered' || step.status === 'verifying');
}
