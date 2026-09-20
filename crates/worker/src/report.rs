//! Turning what the executor observed into what the API is told.
//!
//! One function, and it lives in the library rather than in `bin/worker.rs`
//! for one reason: **a test that reimplements this mapping tests its own copy.**
//!
//! That is not hypothetical. F8 was the API assembling a context, the worker
//! discarding it, and a test on each side asserting about its own half. The
//! live end-to-end proof in `cortex-api/tests/step_end_to_end.rs` has to send
//! a real `StepCompleted` back over a real socket, and if it built that frame
//! by hand it would be asserting that a hand-written frame works — which is
//! precisely the class of coverage that let a step go undispatched for the
//! life of the execution path.
//!
//! So the binary and the test call the same function, and the frame the API
//! grades is the frame the worker would actually have sent.

use cortex_core::protocol::WorkerMessage;
use uuid::Uuid;

use crate::stream::WorkerEvent;

/// Map one executor event onto the wire message the API consumes.
pub fn worker_event_to_message(event: WorkerEvent) -> WorkerMessage {
    match event {
        WorkerEvent::Started {
            step_id,
            attempt_id,
            lease_gen,
            provider,
            model,
            execution_job,
        } => WorkerMessage::StepStarted {
            message_id: Uuid::new_v4().to_string(),
            step_id,
            attempt_id,
            lease_gen,
            provider,
            model,
            execution_job: Some(*execution_job),
        },
        WorkerEvent::Output {
            step_id,
            attempt_id,
            lease_gen,
            line,
        } => WorkerMessage::StepOutput {
            step_id,
            attempt_id,
            lease_gen,
            line,
        },
        WorkerEvent::Completed {
            step_id,
            attempt_id,
            lease_gen,
            exit_code,
            base_commit,
            head_commit,
            branch,
            output,
        } => WorkerMessage::StepCompleted {
            message_id: Uuid::new_v4().to_string(),
            step_id,
            attempt_id,
            lease_gen,
            exit_code,
            base_commit,
            head_commit,
            branch,
            output,
        },
        WorkerEvent::Failed {
            step_id,
            attempt_id,
            lease_gen,
            failure,
        } => WorkerMessage::StepFailed {
            message_id: Uuid::new_v4().to_string(),
            step_id,
            attempt_id,
            lease_gen,
            failure,
        },
        // A refusal travels on its own channel. It used to be a `StepFailed`
        // whose reason was prefixed `BLOCKED:`, because there was no blocked
        // state to transition to and inventing one in PR C would have created
        // a second source of truth for step status. The state exists now, so
        // the prefix is gone: an operator's infrastructure problem is recorded
        // as `execution_failed` against Cortex, not as the customer's step
        // failing.
        WorkerEvent::Blocked {
            step_id,
            attempt_id,
            lease_gen,
            blocked,
        } => WorkerMessage::StepBlocked {
            message_id: Uuid::new_v4().to_string(),
            step_id,
            attempt_id,
            lease_gen,
            blocked,
        },
    }
}
