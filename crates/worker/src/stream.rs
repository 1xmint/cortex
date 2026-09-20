use serde::Serialize;

use cortex_core::execution_job::{Blocked, ExecutionJob};
use cortex_core::failure::WorkerFailureReport;
use cortex_core::protocol::StepOutput;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerEvent {
    Started {
        step_id: String,
        attempt_id: String,
        lease_gen: i64,
        provider: String,
        model: String,
        /// What is about to run. Boxed because it dwarfs every other variant
        /// and an enum is as large as its largest arm.
        execution_job: Box<ExecutionJob>,
    },
    Output {
        step_id: String,
        attempt_id: String,
        lease_gen: i64,
        line: String,
    },
    Completed {
        step_id: String,
        attempt_id: String,
        lease_gen: i64,
        exit_code: i32,
        base_commit: Option<String>,
        head_commit: Option<String>,
        branch: Option<String>,
        output: StepOutput,
    },
    Failed {
        step_id: String,
        attempt_id: String,
        lease_gen: i64,
        failure: WorkerFailureReport,
    },
    /// The step never ran, because the execution boundary could not be
    /// established.
    ///
    /// Distinct from `Failed` on purpose: `Failed` means the work was attempted
    /// and did not succeed, which is a fact about the task. `Blocked` means
    /// Cortex declined to attempt it, which is a fact about Cortex. Collapsing
    /// the two would attribute an operator's infrastructure problem to the
    /// customer's code — and would let a missing sandbox look like a failing
    /// task.
    Blocked {
        step_id: String,
        attempt_id: String,
        lease_gen: i64,
        blocked: Blocked,
    },
}
