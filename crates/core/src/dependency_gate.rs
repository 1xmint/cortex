//! Gating a package at the moment it is added, which is the only moment "do not
//! add it" is still an option.
//!
//! Phase 32.2. An agent resolving a task can add a package, and nothing in the
//! plan gates that. The consequence is specific:
//!
//! > **A green receipt over a run that added a malicious package is a trust
//! > artifact vouching for a compromise.** That is worse than having no receipt,
//! > because the receipt is what persuaded someone not to look.
//!
//! This also corrects Phase 27.2, which classified lockfiles as `incidental`.
//! For *grading* that is right — a lockfile is neither subject nor exam. For
//! *security* it is the highest-risk line in the diff, so dependency changes get
//! their own treatment rather than inheriting `incidental`'s silence.
//!
//! The existing `cargo-audit` and `semgrep` batteries run *after* a dependency
//! is in. These gates run at the moment it is added. Both are cheap; neither was
//! present.
//!
//! # Slopsquatting, and why the near-name gate blocks
//!
//! An agent is precisely the victim a near-name attack targets. A human typing a
//! package name has seen it before; a model emitting one is reconstructing it
//! from a distribution, and the whole attack is to occupy the names that
//! reconstruction lands on.
//!
//! So [`Disposition::Blocked`] is the default for a near-name match, and the
//! false-positive cost is accepted deliberately: blocking a legitimate lookalike
//! costs somebody a conversation, and allowing a squat costs a compromise
//! carrying a receipt that says it was verified. `preact` next to `react` is a
//! real example of the cost, and it has a test.

use serde::{Deserialize, Serialize};

/// A package the agent added, and what is known about it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddedPackage {
    pub name: String,
    /// Whether the registry has it at all.
    pub exists_in_registry: bool,
    /// Days since first publication. `None` when unknown, which is treated as
    /// unknown rather than as old.
    pub age_days: Option<u32>,
    /// Registry attestation or signed provenance, where the ecosystem has it.
    pub has_provenance: bool,
    /// Post-install scripts, build scripts, or native compilation.
    pub runs_install_scripts: bool,
    pub license: Option<String>,
    /// Packages pulled in beyond this direct add.
    pub transitive_additions: u32,
}

/// The organisation's policy on the gates that are policy-dependent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgPolicy {
    /// Block a package with no provenance attestation.
    pub block_without_provenance: bool,
    /// Block a package that executes code at install time.
    pub block_install_scripts: bool,
    /// Allowed licences. Empty means the org has declared none, and the licence
    /// gate then declares rather than blocks.
    pub allowed_licenses: Vec<String>,
    /// A direct add pulling in more than this many transitive packages goes to
    /// review.
    pub transitive_review_threshold: u32,
    /// Below this many days, a package is newly published.
    pub new_package_days: u32,
}

impl Default for OrgPolicy {
    fn default() -> Self {
        Self {
            block_without_provenance: false,
            block_install_scripts: false,
            allowed_licenses: Vec::new(),
            transitive_review_threshold: 10,
            new_package_days: 30,
        }
    }
}

/// What one gate found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "gate")]
pub enum Finding {
    /// Not in the base tree's dependency closure. Declared on the receipt,
    /// always — a new dependency is a fact a reader is entitled to.
    Novel,
    /// The registry does not have it.
    NotInRegistry,
    /// A near-name of a package already present. Slopsquatting.
    NearName {
        resembles: String,
    },
    /// Published recently enough that nobody has looked at it.
    NewlyPublished {
        age_days: Option<u32>,
    },
    NoProvenance,
    RunsInstallScripts,
    LicenceNotAllowed {
        license: Option<String>,
    },
    LargeTransitiveDelta {
        count: u32,
    },
}

/// What happens to the change because of a finding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition {
    /// Recorded on the receipt, and nothing else.
    Declared,
    /// Recorded, and a human looks before it lands.
    RoutedToReview,
    /// The dependency does not go in.
    Blocked,
}

/// One gate's result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateResult {
    pub finding: Finding,
    pub disposition: Disposition,
}

/// The strongest disposition among the findings.
///
/// `Declared` when there are none, because a package that tripped no gate is
/// still declared: novelty is always on the receipt.
pub fn overall(results: &[GateResult]) -> Disposition {
    results
        .iter()
        .map(|r| r.disposition)
        .max()
        .unwrap_or(Disposition::Declared)
}

/// Fold a package name to the form a squatter is exploiting.
///
/// `left-pad`, `left_pad` and `left.pad` are the same name to a reader and
/// different names to a registry, which is the whole trick.
fn normalise(name: &str) -> String {
    name.to_ascii_lowercase()
        .chars()
        .filter(|c| !matches!(c, '-' | '_' | '.'))
        .collect()
}

/// Levenshtein distance, capped: returns `limit + 1` once it is exceeded.
fn distance_within(a: &str, b: &str, limit: usize) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > limit {
        return limit + 1;
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        if current.iter().min().copied().unwrap_or(0) > limit {
            return limit + 1;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// Shorter than this, a one-character difference covers too much of the
/// namespace to be a signal.
const MIN_LENGTH_FOR_EDIT_DISTANCE: usize = 5;

/// The package already present that this name is a near-miss for, if any.
///
/// Two rules. Names that normalise to the same string are always a match —
/// `left-pad` against `left_pad` is not a coincidence. Beyond that, a single
/// edit on a name of at least five characters is a match; below that length the
/// rule would fire on most of the registry.
///
/// An exact match is not a near-name: that is the package already being there.
pub fn near_name<'a>(candidate: &str, present: &'a [String]) -> Option<&'a str> {
    let normalised_candidate = normalise(candidate);
    for existing in present {
        if existing == candidate {
            continue;
        }
        let normalised_existing = normalise(existing);
        if normalised_existing == normalised_candidate {
            return Some(existing);
        }
        if normalised_candidate.len() >= MIN_LENGTH_FOR_EDIT_DISTANCE
            && normalised_existing.len() >= MIN_LENGTH_FOR_EDIT_DISTANCE
            && distance_within(&normalised_candidate, &normalised_existing, 1) <= 1
        {
            return Some(existing);
        }
    }
    None
}

/// Run every gate on one added package.
///
/// `base_closure` is the dependency closure of the base tree — the packages that
/// were already there. A package in it is not novel and is not gated here.
pub fn gate(
    package: &AddedPackage,
    base_closure: &[String],
    policy: &OrgPolicy,
) -> Vec<GateResult> {
    let mut results = Vec::new();

    if base_closure.iter().any(|p| p == &package.name) {
        return results;
    }
    results.push(GateResult {
        finding: Finding::Novel,
        disposition: Disposition::Declared,
    });

    if let Some(resembles) = near_name(&package.name, base_closure) {
        results.push(GateResult {
            finding: Finding::NearName {
                resembles: resembles.to_string(),
            },
            disposition: Disposition::Blocked,
        });
    }

    if !package.exists_in_registry {
        results.push(GateResult {
            finding: Finding::NotInRegistry,
            disposition: Disposition::Blocked,
        });
    }

    // Unknown age is not old age. A package the registry will not date is a
    // package nobody can vouch for the history of.
    let is_new = package.age_days.is_none_or(|d| d < policy.new_package_days);
    if is_new {
        results.push(GateResult {
            finding: Finding::NewlyPublished {
                age_days: package.age_days,
            },
            disposition: Disposition::RoutedToReview,
        });
    }

    if !package.has_provenance {
        results.push(GateResult {
            finding: Finding::NoProvenance,
            disposition: if policy.block_without_provenance {
                Disposition::Blocked
            } else {
                Disposition::Declared
            },
        });
    }

    if package.runs_install_scripts {
        results.push(GateResult {
            finding: Finding::RunsInstallScripts,
            disposition: if policy.block_install_scripts {
                Disposition::Blocked
            } else {
                Disposition::Declared
            },
        });
    }

    if !policy.allowed_licenses.is_empty() {
        let allowed = package
            .license
            .as_ref()
            .is_some_and(|l| policy.allowed_licenses.contains(l));
        if !allowed {
            results.push(GateResult {
                finding: Finding::LicenceNotAllowed {
                    license: package.license.clone(),
                },
                disposition: Disposition::Blocked,
            });
        }
    }

    if package.transitive_additions > policy.transitive_review_threshold {
        results.push(GateResult {
            finding: Finding::LargeTransitiveDelta {
                count: package.transitive_additions,
            },
            disposition: Disposition::RoutedToReview,
        });
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closure(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn clean(name: &str) -> AddedPackage {
        AddedPackage {
            name: name.to_string(),
            exists_in_registry: true,
            age_days: Some(2_000),
            has_provenance: true,
            runs_install_scripts: false,
            license: Some("MIT".to_string()),
            transitive_additions: 0,
        }
    }

    fn findings(results: &[GateResult]) -> Vec<&Finding> {
        results.iter().map(|r| &r.finding).collect()
    }

    #[test]
    fn a_hyphen_for_underscore_swap_is_a_near_name() {
        // The oldest trick in the registry. Same name to a reader, different
        // name to the resolver.
        assert_eq!(
            near_name("left_pad", &closure(&["left-pad"])),
            Some("left-pad")
        );
        assert_eq!(
            near_name("my.package", &closure(&["my-package"])),
            Some("my-package")
        );
    }

    #[test]
    fn a_one_character_substitution_is_a_near_name() {
        assert_eq!(near_name("1odash", &closure(&["lodash"])), Some("lodash"));
        assert_eq!(near_name("reqeusts", &closure(&["requests"])), None);
        assert_eq!(
            near_name("requsts", &closure(&["requests"])),
            Some("requests")
        );
    }

    #[test]
    fn a_package_already_present_is_not_a_near_miss_for_itself() {
        assert_eq!(near_name("lodash", &closure(&["lodash"])), None);
    }

    #[test]
    fn short_names_do_not_trip_the_edit_distance_rule() {
        // At three characters a single edit covers most of the namespace, so
        // the rule would block constantly and get turned off -- which is worse
        // than not having it.
        assert_eq!(near_name("fmt", &closure(&["fnt"])), None);
        // The normalisation rule still applies at any length.
        assert_eq!(near_name("a_b", &closure(&["a-b"])), Some("a-b"));
    }

    #[test]
    fn a_genuinely_similar_package_is_blocked_too_and_that_is_the_trade() {
        // `preact` is a real library one edit from `react`, and this rule
        // blocks it. That cost is accepted deliberately: blocking a legitimate
        // lookalike costs somebody a conversation, and allowing a squat costs a
        // compromise carrying a receipt that says it was verified.
        assert_eq!(near_name("preact", &closure(&["react"])), Some("react"));
    }

    #[test]
    fn an_unrelated_name_sharing_a_prefix_is_not_a_near_name() {
        // `serde_json` next to `serde` must not fire, or every ecosystem's
        // companion crates are unusable.
        assert_eq!(near_name("serde_json", &closure(&["serde"])), None);
        assert_eq!(near_name("tokio-util", &closure(&["tokio"])), None);
    }

    #[test]
    fn a_near_name_blocks() {
        let results = gate(
            &clean("1odash"),
            &closure(&["lodash"]),
            &OrgPolicy::default(),
        );
        assert_eq!(overall(&results), Disposition::Blocked);
        assert!(findings(&results).contains(&&Finding::NearName {
            resembles: "lodash".to_string()
        }));
    }

    #[test]
    fn a_package_already_in_the_base_closure_is_not_gated() {
        // It was already there. Gating it would fire on every lockfile refresh.
        assert!(gate(&clean("serde"), &closure(&["serde"]), &OrgPolicy::default()).is_empty());
    }

    #[test]
    fn novelty_is_declared_even_when_nothing_else_fires() {
        // A new dependency is a fact a reader is entitled to, whatever else is
        // true about it.
        let results = gate(
            &clean("brand-new"),
            &closure(&["serde"]),
            &OrgPolicy::default(),
        );
        assert_eq!(findings(&results), vec![&Finding::Novel]);
        assert_eq!(overall(&results), Disposition::Declared);
    }

    #[test]
    fn an_unknown_age_is_not_treated_as_an_old_package() {
        // A registry that will not date a package is a registry nobody can
        // vouch for its history through. Defaulting unknown to old is how the
        // gate becomes decorative.
        let undated = AddedPackage {
            age_days: None,
            ..clean("mystery")
        };
        let results = gate(&undated, &closure(&[]), &OrgPolicy::default());
        assert!(findings(&results).contains(&&Finding::NewlyPublished { age_days: None }));
        assert_eq!(overall(&results), Disposition::RoutedToReview);
    }

    #[test]
    fn a_missing_package_blocks() {
        let ghost = AddedPackage {
            exists_in_registry: false,
            ..clean("does-not-exist")
        };
        let results = gate(&ghost, &closure(&[]), &OrgPolicy::default());
        assert_eq!(overall(&results), Disposition::Blocked);
    }

    #[test]
    fn provenance_and_install_scripts_declare_by_default_and_block_by_policy() {
        let risky = AddedPackage {
            has_provenance: false,
            runs_install_scripts: true,
            ..clean("builds-natively")
        };
        let lenient = gate(&risky, &closure(&[]), &OrgPolicy::default());
        assert_eq!(overall(&lenient), Disposition::Declared);
        assert!(findings(&lenient).contains(&&Finding::RunsInstallScripts));

        let strict = OrgPolicy {
            block_without_provenance: true,
            block_install_scripts: true,
            ..OrgPolicy::default()
        };
        assert_eq!(
            overall(&gate(&risky, &closure(&[]), &strict)),
            Disposition::Blocked
        );
    }

    #[test]
    fn the_licence_gate_is_silent_until_the_org_declares_one() {
        // "Block where the org has declared one." An empty allowlist is not an
        // allowlist of nothing.
        let gpl = AddedPackage {
            license: Some("GPL-3.0".to_string()),
            ..clean("copyleft")
        };
        assert_eq!(
            overall(&gate(&gpl, &closure(&[]), &OrgPolicy::default())),
            Disposition::Declared
        );

        let declared = OrgPolicy {
            allowed_licenses: vec!["MIT".to_string(), "Apache-2.0".to_string()],
            ..OrgPolicy::default()
        };
        assert_eq!(
            overall(&gate(&gpl, &closure(&[]), &declared)),
            Disposition::Blocked
        );
        assert_eq!(
            overall(&gate(&clean("mit-licensed"), &closure(&[]), &declared)),
            Disposition::Declared
        );
    }

    #[test]
    fn an_unlicensed_package_fails_a_declared_allowlist() {
        let unlicensed = AddedPackage {
            license: None,
            ..clean("no-licence")
        };
        let declared = OrgPolicy {
            allowed_licenses: vec!["MIT".to_string()],
            ..OrgPolicy::default()
        };
        assert_eq!(
            overall(&gate(&unlicensed, &closure(&[]), &declared)),
            Disposition::Blocked
        );
    }

    #[test]
    fn a_large_transitive_delta_routes_to_review() {
        // One direct add pulling in forty packages is a different change from
        // one direct add, and the receipt should not report them the same way.
        let heavy = AddedPackage {
            transitive_additions: 40,
            ..clean("kitchen-sink")
        };
        let results = gate(&heavy, &closure(&[]), &OrgPolicy::default());
        assert_eq!(overall(&results), Disposition::RoutedToReview);
        assert!(findings(&results).contains(&&Finding::LargeTransitiveDelta { count: 40 }));
    }

    #[test]
    fn the_strongest_disposition_wins() {
        // A package can trip several gates, and the receipt reports all of them
        // while the outcome is the worst one.
        let awful = AddedPackage {
            exists_in_registry: true,
            age_days: Some(1),
            has_provenance: false,
            runs_install_scripts: true,
            license: None,
            transitive_additions: 99,
            name: "1odash".to_string(),
        };
        let results = gate(&awful, &closure(&["lodash"]), &OrgPolicy::default());
        assert_eq!(overall(&results), Disposition::Blocked);
        assert!(
            results.len() > 3,
            "every finding is recorded, not just the worst"
        );
    }
}
