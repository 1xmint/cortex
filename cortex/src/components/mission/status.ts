/**
 * The one mapping from backend status words to a visual tone.
 *
 * Its own module rather than living in `ui.tsx`: a components file that also
 * exports helpers breaks fast refresh, and a pure function that decides what
 * a status *means* is worth testing on its own.
 */
export type StatusTone = 'ok' | 'err' | 'warn' | 'busy' | 'idle';

/**
 * One mapping from backend status words to a visual tone. Runs, steps,
 * verifier verdicts, and check outcomes all funnel through here.
 */
export function toneForStatus(status: string | null | undefined): StatusTone {
  switch ((status ?? '').toLowerCase()) {
    // `succeeded` is deliberately absent. A step never reaches it; a run still
    // can, and `completed` covers that.
    case 'completed':
    case 'complete':
    case 'passed':
    case 'verified':
    case 'verified_pass':
    case 'success':
      return 'ok';
    // Delivered and verifying are real, visible conditions — work exists and
    // is being graded. They are not 'ok': nothing has been checked yet.
    case 'delivered':
    case 'verifying':
      return 'busy';
    case 'manual_override':
      return 'warn';
    case 'execution_failed':
    case 'failed':
    case 'cancelled':
    case 'error':
    case 'timed_out':
    case 'blocked':
    case 'verified_fail':
      return 'err';
    case 'needs_evidence':
    case 'inconclusive':
    case 'skipped':
    case 'stale':
    case 'not_executed':
    case 'unknown':
      return 'warn';
    case 'running':
    case 'leased':
    case 'streaming':
    case 'in_progress':
      return 'busy';
    default:
      return 'idle';
  }
}
