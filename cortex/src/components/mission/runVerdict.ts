import type { RunStep } from '../../lib/cortexApi';

// A run's own status can read "completed" while a step underneath it was
// independently verified as `Failed` -- verification finishes after
// execution does. The backend is the real gate (POST .../pr answers 409 for
// exactly this, keyed on the latest sealed verdict, not on whether a refund
// landed), but showing an "Open pull request" button that is certain to be
// refused is its own kind of lie. Same rule the backend applies: any step's
// latest verdict reading `failed` withholds the whole run's PR.
export function hasFailedStep(steps: RunStep[]): boolean {
  return steps.some(
    (step) =>
      step.status === 'failed' ||
      step.verification_status === 'failed' ||
      step.verifier_verdict === 'failed',
  );
}

// A step can be mid-verification -- delivered but not yet graded -- in which
// case its verdict could still land on `Failed`. The backend refuses the PR
// for this too (409 `verification_pending`), so the button should not
// promise something that request would then refuse.
export function hasPendingVerification(steps: RunStep[]): boolean {
  return steps.some((step) => step.status === 'verifying');
}
