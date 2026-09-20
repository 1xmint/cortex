//! The brake pedal, at four scopes, and what a stopped run is allowed to say.
//!
//! Phase 32.1. A system that autonomously spends money, writes code, and pushes
//! branches needs a way to stop, reachable by an operator in seconds. A search
//! of the plan found none of these levers — no kill switch, no circuit breaker,
//! no compensation for external effects. Each is small; collectively they are
//! the difference between an incident and an outage with no brake pedal.
//!
//! | Scope | Reached when |
//! |---|---|
//! | Global | provider incident, sandbox escape signal, ledger anomaly |
//! | Organisation | runaway spend, abuse signal, customer request |
//! | Repository | systematic failures, or a poisoned base |
//! | Task class | a check-derivation or method-library regression |
//!
//! Plus [`Breaker`]s, because the operator is asleep at 3am and every one of
//! these is already a measured number by the time this plan is built.
//!
//! # The two properties that make a stop safe to press
//!
//! **Stopping is a state, not a crash.** Every in-flight item lands in a
//! truthful terminal or resumable state. [`RunOutcome::HaltedByOperator`] is a
//! first-class outcome that refunds, says so, and is resumable where the work is
//! intact. A stop that strands claimed verification jobs recreates the exact bug
//! Phase 1.1 exists to fix.
//!
//! **Stopping is idempotent and reversible.** Pressing it twice is pressing it
//! once — [`StopState::engage`] reports whether anything changed rather than
//! erroring — and releasing it does not stampede every queued run at the
//! provider simultaneously, which is what [`ReleaseRamp`] is for.
//!
//! # The customer-facing half
//!
//! > A halted run says it was **halted by the operator**, not that it failed.
//!
//! Attributing an operator action to the customer's work is the same class of
//! dishonesty as a silent quarantine. [`RunOutcome::attribution`] is the
//! distinction, and it is the reason a halt is a variant of the outcome rather
//! than a `Verdict::Failed` with a note attached.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::verification::Verdict;

/// How wide a stop reaches.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "scope")]
pub enum StopScope {
    /// No new dispatch anywhere.
    Global,
    /// No new dispatch for this organisation.
    Organisation { org: String },
    /// No new dispatch against this repository.
    Repository { org: String, repo: String },
    /// This task class only, across every organisation — a check-derivation or
    /// method-library regression is not one customer's problem.
    TaskClass { class: String },
}

/// What a dispatch would be for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchTarget {
    pub org: String,
    pub repo: String,
    pub task_class: String,
}

/// Which stops are currently engaged.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopState {
    engaged: BTreeSet<StopScope>,
}

impl StopState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Engage a stop. Returns whether this changed anything.
    ///
    /// Pressing it twice is pressing it once. An operator hitting the button
    /// again because the page had not refreshed must not get an error, and must
    /// not get a second stop to release.
    pub fn engage(&mut self, scope: StopScope) -> bool {
        self.engaged.insert(scope)
    }

    /// Release a stop. Returns whether this changed anything.
    pub fn release(&mut self, scope: &StopScope) -> bool {
        self.engaged.remove(scope)
    }

    pub fn engaged(&self) -> impl Iterator<Item = &StopScope> {
        self.engaged.iter()
    }

    /// The stop preventing this dispatch, if any.
    ///
    /// `Global` is reported ahead of anything else, because an operator reading
    /// "this repository is stopped" while everything is stopped would go and fix
    /// the wrong thing. Beyond that the order is the enum's, which is stable and
    /// not a containment order — a task-class stop crosses every organisation
    /// while a repository stop does not, so neither is "wider". Use
    /// [`engaged`](Self::engaged) when the full set matters.
    pub fn blocking(&self, target: &DispatchTarget) -> Option<&StopScope> {
        self.engaged.iter().find(|scope| match scope {
            StopScope::Global => true,
            StopScope::Organisation { org } => org == &target.org,
            StopScope::Repository { org, repo } => org == &target.org && repo == &target.repo,
            StopScope::TaskClass { class } => class == &target.task_class,
        })
    }

    pub fn permits(&self, target: &DispatchTarget) -> bool {
        self.blocking(target).is_none()
    }
}

/// A condition that engages a stop without an operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Breaker {
    /// Loss ratio crossing a threshold within a window.
    LossRatio,
    /// Verified-then-reverted rate spiking — the live false-accept signal.
    VerifiedThenReverted,
    /// Spend rate for an org exceeding its own trailing baseline by a multiple.
    SpendRateAboveBaseline,
    /// Provider error or latency degradation — already detected, and previously
    /// wired to nothing that stops.
    ProviderDegradation,
    /// Any sandbox integrity assertion failing.
    SandboxIntegrity,
}

impl Breaker {
    /// How wide this breaker trips.
    ///
    /// The two that indicate the *system* is wrong rather than one customer's
    /// work go global. A sandbox integrity failure is not a per-repository
    /// problem, and neither is a provider outage.
    pub fn scope_for(&self, org: &str) -> StopScope {
        match self {
            Self::SandboxIntegrity | Self::ProviderDegradation => StopScope::Global,
            Self::LossRatio | Self::VerifiedThenReverted | Self::SpendRateAboveBaseline => {
                StopScope::Organisation {
                    org: org.to_string(),
                }
            }
        }
    }
}

/// Who an outcome is attributable to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Attribution {
    /// The customer's work produced this.
    CustomerWork,
    /// Cortex did this to the run.
    Operator,
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RunOutcome {
    /// The run finished and was graded.
    Completed { verdict: Verdict },
    /// An operator or a breaker stopped it.
    HaltedByOperator {
        scope: StopScope,
        /// The delivered work is intact and the run can pick up where it
        /// stopped.
        resumable: bool,
    },
}

impl RunOutcome {
    /// Whether money goes back to the customer.
    ///
    /// A halt always refunds: the customer did not get what they paid for, and
    /// the reason was ours. `Inconclusive` does not, because nothing was
    /// charged in the first place.
    pub fn refunds(&self) -> bool {
        match self {
            Self::HaltedByOperator { .. } => true,
            Self::Completed { verdict } => matches!(verdict, Verdict::Failed),
        }
    }

    /// Who this outcome is about.
    ///
    /// The honesty property, and the reason a halt is its own variant rather
    /// than a `Failed` with a note. Attributing an operator action to the
    /// customer's work is the same class of dishonesty as a silent quarantine.
    pub fn attribution(&self) -> Attribution {
        match self {
            Self::HaltedByOperator { .. } => Attribution::Operator,
            Self::Completed { .. } => Attribution::CustomerWork,
        }
    }
}

/// How queued work resumes after a release.
///
/// Releasing a global stop with ten thousand runs behind it and no metering
/// sends ten thousand requests at a provider that may be the reason the stop
/// was engaged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseRamp {
    pub queued: u32,
    per_tick: u32,
}

impl ReleaseRamp {
    /// `None` for a rate of zero, which would never drain and is a
    /// configuration mistake rather than a very cautious ramp.
    pub fn new(queued: u32, per_tick: u32) -> Option<Self> {
        if per_tick == 0 {
            return None;
        }
        Some(Self { queued, per_tick })
    }

    pub fn per_tick(&self) -> u32 {
        self.per_tick
    }

    /// Ticks before the backlog is drained.
    pub fn ticks_to_drain(&self) -> u32 {
        self.queued.div_ceil(self.per_tick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(org: &str, repo: &str, class: &str) -> DispatchTarget {
        DispatchTarget {
            org: org.to_string(),
            repo: repo.to_string(),
            task_class: class.to_string(),
        }
    }

    #[test]
    fn a_global_stop_permits_nothing() {
        let mut state = StopState::new();
        state.engage(StopScope::Global);
        assert!(!state.permits(&target("acme", "api", "refactor")));
        assert!(!state.permits(&target("globex", "web", "heal")));
    }

    #[test]
    fn an_organisation_stop_leaves_other_organisations_alone() {
        let mut state = StopState::new();
        state.engage(StopScope::Organisation {
            org: "acme".to_string(),
        });
        assert!(!state.permits(&target("acme", "api", "refactor")));
        assert!(state.permits(&target("globex", "api", "refactor")));
    }

    #[test]
    fn a_repository_stop_leaves_the_organisations_other_repositories_alone() {
        let mut state = StopState::new();
        state.engage(StopScope::Repository {
            org: "acme".to_string(),
            repo: "poisoned".to_string(),
        });
        assert!(!state.permits(&target("acme", "poisoned", "heal")));
        assert!(state.permits(&target("acme", "healthy", "heal")));
        assert!(state.permits(&target("globex", "poisoned", "heal")));
    }

    #[test]
    fn a_task_class_stop_crosses_every_organisation() {
        // A check-derivation or method-library regression is not one customer's
        // problem, so scoping it to one org would leave it running everywhere
        // else.
        let mut state = StopState::new();
        state.engage(StopScope::TaskClass {
            class: "refactor".to_string(),
        });
        assert!(!state.permits(&target("acme", "api", "refactor")));
        assert!(!state.permits(&target("globex", "web", "refactor")));
        assert!(state.permits(&target("acme", "api", "heal")));
    }

    #[test]
    fn pressing_stop_twice_is_pressing_it_once() {
        // An operator hitting the button again because the page had not
        // refreshed must not get an error, and must not create a second stop to
        // remember to release.
        let mut state = StopState::new();
        assert!(state.engage(StopScope::Global));
        assert!(!state.engage(StopScope::Global));
        assert_eq!(state.engaged().count(), 1);

        assert!(state.release(&StopScope::Global));
        assert!(!state.release(&StopScope::Global));
        assert!(state.permits(&target("acme", "api", "heal")));
    }

    #[test]
    fn releasing_one_scope_leaves_the_others_engaged() {
        let mut state = StopState::new();
        state.engage(StopScope::Global);
        state.engage(StopScope::Organisation {
            org: "acme".to_string(),
        });
        state.release(&StopScope::Global);
        assert!(!state.permits(&target("acme", "api", "heal")));
        assert!(state.permits(&target("globex", "api", "heal")));
    }

    #[test]
    fn a_global_stop_is_reported_ahead_of_any_narrower_one() {
        // An operator reading "this repository is stopped" while everything is
        // stopped would go and fix the wrong thing. This is the only precedence
        // claim made -- the rest of the order is stable rather than meaningful,
        // because a task-class stop and a repository stop do not contain each
        // other.
        let mut state = StopState::new();
        state.engage(StopScope::Repository {
            org: "acme".to_string(),
            repo: "api".to_string(),
        });
        state.engage(StopScope::TaskClass {
            class: "heal".to_string(),
        });
        state.engage(StopScope::Global);
        assert_eq!(
            state.blocking(&target("acme", "api", "heal")),
            Some(&StopScope::Global)
        );
    }

    #[test]
    fn every_engaged_stop_is_readable_even_though_one_is_reported() {
        // `blocking` answers "why is this not dispatching"; releasing needs the
        // whole set, and releasing only the reported one would leave dispatch
        // still stopped for a reason the operator thought they had cleared.
        let mut state = StopState::new();
        state.engage(StopScope::Global);
        state.engage(StopScope::TaskClass {
            class: "heal".to_string(),
        });
        assert_eq!(state.engaged().count(), 2);
        state.release(&StopScope::Global);
        assert_eq!(
            state.blocking(&target("acme", "api", "heal")),
            Some(&StopScope::TaskClass {
                class: "heal".to_string()
            })
        );
    }

    #[test]
    fn a_halt_is_not_the_customers_work_failing() {
        // The honesty property. Attributing an operator action to the
        // customer's work is the same class of dishonesty as a silent
        // quarantine, which is why this is its own variant rather than a
        // `Failed` with a note.
        let halted = RunOutcome::HaltedByOperator {
            scope: StopScope::Global,
            resumable: true,
        };
        assert_eq!(halted.attribution(), Attribution::Operator);
        assert_eq!(
            RunOutcome::Completed {
                verdict: Verdict::Failed
            }
            .attribution(),
            Attribution::CustomerWork
        );
    }

    #[test]
    fn a_halt_always_refunds() {
        // Whatever the scope, whatever the reason, and whether or not the work
        // survives. The customer did not get what they paid for and the reason
        // was ours.
        for scope in [
            StopScope::Global,
            StopScope::Organisation {
                org: "acme".to_string(),
            },
            StopScope::TaskClass {
                class: "heal".to_string(),
            },
        ] {
            for resumable in [true, false] {
                assert!(RunOutcome::HaltedByOperator {
                    scope: scope.clone(),
                    resumable
                }
                .refunds());
            }
        }
    }

    #[test]
    fn an_inconclusive_verdict_does_not_refund_because_nothing_was_charged() {
        assert!(!RunOutcome::Completed {
            verdict: Verdict::Inconclusive
        }
        .refunds());
        assert!(!RunOutcome::Completed {
            verdict: Verdict::Verified
        }
        .refunds());
        assert!(RunOutcome::Completed {
            verdict: Verdict::Failed
        }
        .refunds());
    }

    #[test]
    fn sandbox_and_provider_breakers_trip_globally() {
        // Neither is one customer's problem, and scoping them to an
        // organisation would leave a sandbox escape running everywhere else.
        assert_eq!(
            Breaker::SandboxIntegrity.scope_for("acme"),
            StopScope::Global
        );
        assert_eq!(
            Breaker::ProviderDegradation.scope_for("acme"),
            StopScope::Global
        );
    }

    #[test]
    fn commercial_breakers_trip_at_the_organisation() {
        for breaker in [
            Breaker::LossRatio,
            Breaker::VerifiedThenReverted,
            Breaker::SpendRateAboveBaseline,
        ] {
            assert_eq!(
                breaker.scope_for("acme"),
                StopScope::Organisation {
                    org: "acme".to_string()
                },
                "{breaker:?} is about one organisation's book"
            );
        }
    }

    #[test]
    fn a_release_is_metered_so_it_does_not_stampede() {
        // Releasing a global stop with ten thousand runs behind it and no
        // metering sends ten thousand requests at a provider that may be the
        // reason the stop was engaged.
        let ramp = ReleaseRamp::new(10_000, 50).unwrap();
        assert_eq!(ramp.ticks_to_drain(), 200);
        assert_eq!(ReleaseRamp::new(101, 50).unwrap().ticks_to_drain(), 3);
        assert_eq!(ReleaseRamp::new(0, 50).unwrap().ticks_to_drain(), 0);
    }

    #[test]
    fn a_ramp_that_never_drains_is_refused() {
        // Zero per tick is a configuration mistake, not a very cautious ramp.
        assert!(ReleaseRamp::new(100, 0).is_none());
    }

    #[test]
    fn nothing_engaged_permits_everything() {
        let state = StopState::new();
        assert!(state.permits(&target("acme", "api", "heal")));
        assert_eq!(state.engaged().count(), 0);
    }
}
