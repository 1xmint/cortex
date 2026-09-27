import type { RunStep } from '../../lib/cortexApi';

// A run's own status can read "completed" while a step underneath it was
// independently verified as failed and refunded -- verification finishes
// after execution does. The backend is the real gate (POST .../pr answers
// 409 for exactly this), but showing an "Open pull request" button that is
// certain to be refused, for work the customer was already refunded for, is
// its own kind of lie. Same rule the backend applies: any step's terminal
// verdict reading `failed` withholds the whole run's PR.
export function hasFailedRefundedStep(steps: RunStep[]): boolean {
  return steps.some(
    (step) =>
      step.status === 'failed' ||
      step.verification_status === 'failed' ||
      step.verifier_verdict === 'failed',
  );
}
