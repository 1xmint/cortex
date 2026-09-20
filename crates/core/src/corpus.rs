//! The corpus nobody else can build, and the boundary it may never cross.
//!
//! Phase 29.4. This is the compounding mechanism Phase 28.2(d) deferred, and a
//! stronger moat than the professional system because it accrues automatically
//! from work Cortex is already paid to do.
//!
//! The measured result worth building toward: **accurately summarised and
//! retrieved prior experience improves resolution accuracy while simultaneously
//! reducing runtime and token cost, and the gain is largest on hard tasks.**
//! Better and cheaper at once is rare. It holds here because most of what an
//! agent spends on a hard task is re-deriving orientation someone already paid
//! for.
//!
//! What Cortex holds per repository that no model vendor and no IDE does:
//!
//! | Asset | Where it comes from | What it answers next time |
//! |---|---|---|
//! | `TaskFrame` → files actually changed | every completed run | which subsystem does this *kind* of request touch here |
//! | derived battery, its cost, its determinism record | Phase 24.1, 27.3 | how is this repo verified, and what can be trusted |
//! | checks that failed and the fix that passed them | verification history | the repo's recurring failure modes |
//! | review findings and their dispositions | Phase 3.3, Phase 12 | what this team rejects, in this repo |
//! | post-delivery reverts | Phase 26.3 | where verified work is nonetheless wrong here |
//! | race-attempt disagreement | Phase 27.4 | which regions of this repo are underspecified |
//!
//! The last row is quietly the most interesting. Regions where independent
//! attempts systematically disagree are regions where the repository does not
//! constrain behaviour — a real, sellable finding about the codebase that arrives
//! as a by-product of racing.
//!
//! # Tenancy is the hard boundary, and it is architectural here
//!
//! > A retrieved-experience layer that leaks across tenants is a source-code
//! > disclosure with extra steps, and it must be **architecturally impossible
//! > rather than policy-prevented**.
//!
//! Two types, and no bridge between them.
//!
//! [`RepositoryExperience`] carries a tenant and a repo and has free-text
//! claims. It is everything derived from a customer's code, findings, or
//! dispositions, and [`retrievable`] will only hand it to the exact
//! `(tenant, repo)` it was learned in.
//!
//! [`EcosystemFact`] is a **closed enum of objective shapes** — a conventional
//! test command, an entry-point pattern, an affected-target query. There is no
//! variant that can hold a claim about a customer's code, so the thing that must
//! not generalise has nowhere to live. That is the difference between this and a
//! shared table with a `tenant_id` column and a filter someone can forget: here
//! the leak is not prevented, it is unrepresentable.
//!
//! There is deliberately no function converting one into the other. Promoting a
//! repository observation to ecosystem scope is exactly the disclosure, and it
//! should require writing a new variant in this file rather than calling a
//! method.

use serde::{Deserialize, Serialize};

/// What kind of prior experience this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperienceKind {
    /// Which subsystem a kind of request touches in this repository.
    SubsystemForRequestKind,
    /// How this repository is verified, and what that costs.
    BatteryShape,
    /// A failure mode that keeps coming back here.
    RecurringFailure,
    /// What this team rejects in review.
    TeamDisposition,
    /// Where verified work turned out to be wrong anyway.
    PostDeliveryRevert,
    /// A region independent attempts systematically disagree about — which is
    /// to say, a region the repository does not constrain.
    UnderspecifiedRegion,
}

/// Something learned about one customer's repository.
///
/// Every field of this is derived from a customer's code, findings, or
/// dispositions, so the type carries its tenancy and never loses it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryExperience {
    tenant: String,
    repo: String,
    pub kind: ExperienceKind,
    pub claim: String,
}

impl RepositoryExperience {
    pub fn new(
        tenant: impl Into<String>,
        repo: impl Into<String>,
        kind: ExperienceKind,
        claim: impl Into<String>,
    ) -> Self {
        Self {
            tenant: tenant.into(),
            repo: repo.into(),
            kind,
            claim: claim.into(),
        }
    }

    /// Whether this may be shown to a run in `(tenant, repo)`.
    ///
    /// Exact match on both. Same repo name under a different organisation is a
    /// different repository and has nothing to do with this one — which sounds
    /// obvious and is the exact shape of the mistake, because `api`, `web` and
    /// `backend` are the same name in every organisation on earth.
    pub fn readable_by(&self, tenant: &str, repo: &str) -> bool {
        self.tenant == tenant && self.repo == repo
    }

    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    pub fn repo(&self) -> &str {
        &self.repo
    }
}

/// A structural, objective fact about an ecosystem — safe to share because
/// there is no shape here that can carry a customer's code.
///
/// Closed on purpose. Adding a variant is the review point where someone has to
/// argue that the new shape is objective, and that argument is much easier to
/// have about an enum variant than about a row in a table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EcosystemFact {
    /// What this ecosystem conventionally runs to test. Argv, never a shell
    /// string.
    ConventionalTestCommand {
        ecosystem: String,
        command: Vec<String>,
    },
    /// Where this framework usually starts.
    EntryPointPattern { framework: String, pattern: String },
    /// How this build system is asked which targets a change affects.
    AffectedTargetQuery {
        build_system: String,
        query: Vec<String>,
    },
}

/// The prior experience a run in `(tenant, repo)` may be shown.
///
/// The filter is on the entries themselves rather than on a query, because a
/// query filter is a thing a caller can forget to apply and this is not.
pub fn retrievable<'a>(
    experiences: &'a [RepositoryExperience],
    tenant: &str,
    repo: &str,
) -> Vec<&'a RepositoryExperience> {
    experiences
        .iter()
        .filter(|e| e.readable_by(tenant, repo))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn experience(tenant: &str, repo: &str, claim: &str) -> RepositoryExperience {
        RepositoryExperience::new(tenant, repo, ExperienceKind::RecurringFailure, claim)
    }

    #[test]
    fn the_same_repo_name_under_another_organisation_shares_nothing() {
        // The shape of the mistake. `api`, `web` and `backend` are the same
        // name in every organisation on earth, so a corpus keyed on repo name
        // hands one customer another's failure modes.
        let corpus = [
            experience("acme", "api", "the auth tests are flaky on CI"),
            experience("globex", "api", "migrations must run before the suite"),
        ];

        let acme = retrievable(&corpus, "acme", "api");
        assert_eq!(acme.len(), 1);
        assert_eq!(acme[0].claim, "the auth tests are flaky on CI");

        let globex = retrievable(&corpus, "globex", "api");
        assert_eq!(globex.len(), 1);
        assert_eq!(globex[0].claim, "migrations must run before the suite");
    }

    #[test]
    fn one_tenants_other_repository_is_still_another_repository() {
        let corpus = [experience("acme", "api", "x")];
        assert!(retrievable(&corpus, "acme", "web").is_empty());
    }

    #[test]
    fn an_unknown_tenant_retrieves_nothing() {
        let corpus = [
            experience("acme", "api", "x"),
            experience("globex", "web", "y"),
        ];
        assert!(retrievable(&corpus, "initech", "api").is_empty());
        assert!(retrievable(&corpus, "", "").is_empty());
    }

    #[test]
    fn every_experience_kind_is_tenant_scoped() {
        // Including the ones that look generic. "This team rejects wide
        // try/catch" reads like an engineering opinion and is a disposition
        // learned from one customer's review history.
        for kind in [
            ExperienceKind::SubsystemForRequestKind,
            ExperienceKind::BatteryShape,
            ExperienceKind::RecurringFailure,
            ExperienceKind::TeamDisposition,
            ExperienceKind::PostDeliveryRevert,
            ExperienceKind::UnderspecifiedRegion,
        ] {
            let e = RepositoryExperience::new("acme", "api", kind, "something learned");
            assert!(e.readable_by("acme", "api"));
            assert!(
                !e.readable_by("globex", "api"),
                "{kind:?} must not cross an organisation"
            );
        }
    }

    #[test]
    fn a_race_disagreement_is_a_finding_about_the_repository() {
        // The quietly interesting asset: regions where independent attempts
        // systematically disagree are regions the repository does not
        // constrain. It arrives as a by-product of racing and is worth selling.
        let e = RepositoryExperience::new(
            "acme",
            "api",
            ExperienceKind::UnderspecifiedRegion,
            "three attempts chose three different retry policies in src/http",
        );
        assert_eq!(e.kind, ExperienceKind::UnderspecifiedRegion);
        assert!(e.readable_by("acme", "api"));
    }

    #[test]
    fn an_ecosystem_fact_has_nowhere_to_put_a_tenant() {
        // The architectural half. These variants describe a build system, not a
        // customer, and the type has no field a repository observation could be
        // smuggled through. Serialising one shows what a shared row can hold.
        let fact = EcosystemFact::ConventionalTestCommand {
            ecosystem: "cargo".to_string(),
            command: vec!["cargo".to_string(), "test".to_string()],
        };
        let json = serde_json::to_value(&fact).unwrap();
        assert_eq!(json["kind"], "conventional_test_command");
        assert_eq!(json["ecosystem"], "cargo");
        assert!(
            json.get("tenant").is_none() && json.get("repo").is_none(),
            "a shareable fact must not carry a tenancy it could be filtered by"
        );
    }

    #[test]
    fn the_shareable_shapes_are_all_objective() {
        // Each of these is checkable by anyone with the toolchain and nobody's
        // source. That is the test for whether a shape belongs in this enum,
        // and adding a variant is where someone has to make that argument.
        let facts = [
            EcosystemFact::ConventionalTestCommand {
                ecosystem: "npm".to_string(),
                command: vec!["npm".to_string(), "test".to_string()],
            },
            EcosystemFact::EntryPointPattern {
                framework: "axum".to_string(),
                pattern: "Router::new".to_string(),
            },
            EcosystemFact::AffectedTargetQuery {
                build_system: "bazel".to_string(),
                query: vec!["bazel".to_string(), "query".to_string()],
            },
        ];
        for fact in &facts {
            let json = serde_json::to_value(fact).unwrap();
            assert!(json.get("tenant").is_none());
        }
    }

    #[test]
    fn an_empty_corpus_retrieves_nothing_and_does_not_panic() {
        assert!(retrievable(&[], "acme", "api").is_empty());
    }
}
