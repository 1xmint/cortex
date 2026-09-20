//! What a registry grant *means*, and who decides a step gets one.
//!
//! Enforcement lives at the sandbox edge in `cortex-worker`. This module is the
//! half that has to be shared, for a reason worth stating: the planning path
//! issues the grant and names the allowlist, and the worker refuses any
//! allowlist entry no grant justifies. Those two ends must agree about what
//! `"npm"` expands to. If each kept its own table they would agree until one of
//! them was edited, and the failure mode is silent — the sandbox opens strictly
//! less than the planner believed, so a task fails looking like a broken
//! network rather than like a policy mismatch.
//!
//! So the table lives here, once, and both ends read it.
//!
//! There are **two** derivations in this module and they are deliberately not
//! one. [`derive_egress`] answers "what does this repository's ecosystem
//! justify" and reads manifests. [`derive_provider_egress`] answers "which
//! model API did the router send this step to" and reads the routing decision.
//! Merging them would let a file in a customer's repository influence which
//! provider the sandbox can reach, which is a different question with a much
//! worse wrong answer. They meet only at [`EgressPlan::union`], and only to
//! build the one allowlist a sandbox can have.

use serde::{Deserialize, Serialize};

use crate::execution_job::{CapabilityGrant, NetworkPolicy};
use crate::provider::ProviderId;

/// What a registry name expands to.
///
/// A grant names a registry; **we** decide what that name reaches. This is what
/// "package-registry access means exactly the registry" is in code: a task
/// cannot widen its own grant by naming an extra hostname, because a name this
/// table does not know expands to nothing.
///
/// Every host here is required for the ecosystem's default client to resolve
/// and download a dependency, and nothing here is required for anything else.
pub const REGISTRIES: &[(&str, &[&str])] = &[
    // `npm install` resolves metadata and tarballs from the same host.
    ("npm", &["registry.npmjs.org"]),
    // cargo reads the sparse index from index.crates.io and downloads .crate
    // files from static.crates.io. crates.io itself is the API, used by
    // `cargo publish` and `cargo search`.
    (
        "crates",
        &["index.crates.io", "static.crates.io", "crates.io"],
    ),
    // pip resolves from pypi.org and downloads wheels from the file host.
    ("pypi", &["pypi.org", "files.pythonhosted.org"]),
    // The module proxy serves modules; the checksum database is what makes a
    // module verifiable, so granting one without the other breaks `go mod`.
    ("go", &["proxy.golang.org", "sum.golang.org"]),
];

/// The hosts a registry name justifies, or `None` if we do not know the name.
///
/// A raw hostname is accepted when it is one this table already knows — so a
/// grant of `crates.io` still means `crates.io` — but a hostname the table has
/// never heard of justifies nothing.
pub fn expand_registry(name: &str) -> Option<Vec<String>> {
    let name = name.trim().to_ascii_lowercase();
    if let Some((_, hosts)) = REGISTRIES.iter().find(|(alias, _)| *alias == name) {
        return Some(hosts.iter().map(|host| host.to_string()).collect());
    }
    if REGISTRIES
        .iter()
        .any(|(_, hosts)| hosts.iter().any(|host| *host == name))
    {
        return Some(vec![name]);
    }
    None
}

/// The single API host each provider's CLI must reach to do any work at all.
///
/// One host per provider, and the entry is the whole grant. This is the same
/// shape as [`REGISTRIES`] and for the same reason: a grant names the
/// *provider*, and this table — not the grant, and certainly not the task —
/// decides what that name reaches. A grant naming a provider this table does
/// not know expands to nothing, so an unknown name opens nothing rather than
/// being trusted as a hostname.
///
/// Each host is where the corresponding CLI sends its completion requests, and
/// nothing here is required for anything else. Telemetry, update checks and
/// crash reporting endpoints are deliberately absent: a CLI that cannot phone
/// home still does the work.
pub const PROVIDER_ENDPOINTS: &[(ProviderId, &str)] = &[
    // Claude Code talks to the request-forwarding gateway. The supplier host
    // is deliberately absent from the sandbox allowlist.
    (ProviderId::Claude, "cortex.heyvera.org"),
    (ProviderId::Openai, "api.openai.com"),
    (ProviderId::Gemini, "generativelanguage.googleapis.com"),
    // Zen is API-only and has no CLI, so no sandboxed step is ever routed to
    // it — `build_command` rejects it before a sandbox is built. The entry
    // exists so this table stays exhaustive over `ProviderId` rather than
    // silently acquiring a hole the day Zen grows a CLI.
    (ProviderId::Zen, "opencode.ai"),
];

/// The wire name for a provider in a [`CapabilityGrant::ReachProvider`].
///
/// Deliberately the `serde` spelling of [`ProviderId`], so the grant persisted
/// on a job round-trips to the same provider that was routed.
pub fn provider_grant_name(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Claude => "claude",
        ProviderId::Openai => "openai",
        ProviderId::Gemini => "gemini",
        ProviderId::Zen => "zen",
    }
}

/// The host a provider name justifies, or `None` if we do not know the name.
///
/// The inverse of [`provider_grant_name`] followed by a table lookup, kept as
/// one function because the two halves must not be able to disagree.
pub fn expand_provider(name: &str) -> Option<&'static str> {
    let name = name.trim().to_ascii_lowercase();
    PROVIDER_ENDPOINTS
        .iter()
        .find(|(provider, _)| provider_grant_name(*provider) == name)
        .map(|(_, host)| *host)
}

/// The API host this provider's CLI reaches.
pub fn provider_host(provider: ProviderId) -> Option<&'static str> {
    PROVIDER_ENDPOINTS
        .iter()
        .find(|(candidate, _)| *candidate == provider)
        .map(|(_, host)| *host)
}

/// Decide the provider egress for one step, from the routing decision alone.
///
/// The input is a `ProviderId` and nothing else. That is the security property,
/// not an implementation convenience: this function **cannot** be influenced by
/// the repository, the task contract, the objective, or the step kind, because
/// none of them is in scope. A step routed to one provider gets exactly that
/// provider's host; there is no argument reachable from a customer's repository
/// that adds a second one.
///
/// The planner already made this decision — it chose the model — so this is a
/// restatement of a choice already taken, not a new one.
pub fn derive_provider_egress(provider: ProviderId) -> EgressPlan {
    let Some(host) = provider_host(provider) else {
        return EgressPlan::deny();
    };

    EgressPlan {
        network_policy: NetworkPolicy::Allowlist {
            hosts: vec![host.to_string()],
        },
        capability_grants: vec![CapabilityGrant::ReachProvider {
            provider: provider_grant_name(provider).to_string(),
        }],
    }
}

/// Which dependency manifests a repository actually contains.
///
/// Deliberately separate from `check_derivation::EcosystemFacts`, which decides
/// what must *pass*. The two questions have different answers — a repo with a
/// `go.mod` needs the Go module proxy but contributes nothing to the check
/// floor — and keeping them apart means a change to what egress opens cannot
/// change what verification requires, or the reverse.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EcosystemManifests {
    pub cargo: bool,
    pub npm: bool,
    pub pypi: bool,
    pub go: bool,
}

impl EcosystemManifests {
    pub fn is_empty(&self) -> bool {
        !self.cargo && !self.npm && !self.pypi && !self.go
    }

    /// The registry aliases these manifests justify, in a stable order.
    fn registries(&self) -> Vec<String> {
        let mut names = Vec::new();
        if self.cargo {
            names.push("crates".to_string());
        }
        if self.npm {
            names.push("npm".to_string());
        }
        if self.pypi {
            names.push("pypi".to_string());
        }
        if self.go {
            names.push("go".to_string());
        }
        names
    }
}

/// A grant and the allowlist that carries it, decided at plan time.
///
/// Both halves travel together because the worker requires both: a grant with
/// no allowlist entry opens nothing, and an allowlist entry with no grant is
/// refused and reported. Deriving them in one function is what keeps them from
/// disagreeing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressPlan {
    pub network_policy: NetworkPolicy,
    pub capability_grants: Vec<CapabilityGrant>,
}

impl EgressPlan {
    /// The plan that opens nothing. This is the default, and it is what every
    /// path that cannot positively justify a grant must return.
    pub fn deny() -> Self {
        Self {
            network_policy: NetworkPolicy::Deny,
            capability_grants: Vec::new(),
        }
    }

    pub fn is_deny(&self) -> bool {
        self.network_policy.is_deny()
    }

    /// The registry aliases granted, for a receipt or a log line.
    pub fn granted_registries(&self) -> Vec<String> {
        self.capability_grants
            .iter()
            .flat_map(|grant| match grant {
                CapabilityGrant::ResolveDependencies { registries } => registries.clone(),
                CapabilityGrant::ReachProvider { .. } | CapabilityGrant::ReadSecret { .. } => {
                    Vec::new()
                }
            })
            .collect()
    }

    /// The provider granted, if any. At most one — see [`union`](Self::union).
    pub fn granted_provider(&self) -> Option<&str> {
        self.capability_grants.iter().find_map(|grant| match grant {
            CapabilityGrant::ReachProvider { provider } => Some(provider.as_str()),
            CapabilityGrant::ResolveDependencies { .. } | CapabilityGrant::ReadSecret { .. } => {
                None
            }
        })
    }

    /// Combine two independently derived plans into the one allowlist a sandbox
    /// can have.
    ///
    /// This is the *only* place the ecosystem and provider derivations meet,
    /// and it meets them as late as possible on purpose. Each was decided from
    /// its own inputs — manifests for one, the routing decision for the other —
    /// and neither could see the other's. Union here changes what is reachable;
    /// it cannot change what either side decided, because both are already
    /// finished values by the time they arrive.
    ///
    /// The grants stay as they were derived rather than being flattened into
    /// one list of hosts. That is what keeps a receipt able to say *why* each
    /// host was open: a reader can tell a registry grant from a provider grant
    /// after the fact, which a merged host list would have destroyed.
    ///
    /// Two provider grants would be a bug in the caller — a step has one
    /// routing decision — so the first wins and the second is dropped rather
    /// than quietly widening the allowlist to two providers.
    pub fn union(ecosystem: &EgressPlan, provider: &EgressPlan) -> EgressPlan {
        let mut hosts: Vec<String> = ecosystem
            .network_policy
            .allowed_hosts()
            .iter()
            .chain(provider.network_policy.allowed_hosts())
            .cloned()
            .collect();
        hosts.sort();
        hosts.dedup();

        if hosts.is_empty() {
            return EgressPlan::deny();
        }

        let mut capability_grants = ecosystem.capability_grants.clone();
        if let Some(grant) = provider
            .capability_grants
            .iter()
            .find(|grant| matches!(grant, CapabilityGrant::ReachProvider { .. }))
        {
            capability_grants.push(grant.clone());
        }

        EgressPlan {
            network_policy: NetworkPolicy::Allowlist { hosts },
            capability_grants,
        }
    }
}

/// Decide the egress for one step.
///
/// Two conditions, both required, and the order they are checked in is the
/// argument:
///
/// 1. **The step must be able to change the tree.** A Search, Think, Review or
///    Gate step reads and reasons; it never resolves a dependency. Granting it
///    a registry would open a network for work that has no use for one, which
///    is exactly the "just in case" grant that makes an allowlist decorative.
/// 2. **The repository must actually use the ecosystem.** A grant is justified
///    by a manifest on disk, not by a guess about what a task might want. A
///    repository with no `Cargo.toml` gets no route to crates.io no matter what
///    the task says it needs.
///
/// Anything else is [`EgressPlan::deny`]. There is no argument that can widen
/// this from inside the task: the input is the repository and the step kind,
/// neither of which the task authors.
pub fn derive_egress(manifests: &EcosystemManifests, step_changes_tree: bool) -> EgressPlan {
    if !step_changes_tree || manifests.is_empty() {
        return EgressPlan::deny();
    }

    let registries = manifests.registries();
    let mut hosts: Vec<String> = registries
        .iter()
        .filter_map(|name| expand_registry(name))
        .flatten()
        .collect();
    hosts.sort();
    hosts.dedup();

    // Belt and braces: if the table somehow expanded nothing, deny rather than
    // emit an `Allowlist { hosts: [] }`, which reads as a policy that permits
    // something and permits nothing.
    if hosts.is_empty() {
        return EgressPlan::deny();
    }

    EgressPlan {
        network_policy: NetworkPolicy::Allowlist { hosts },
        capability_grants: vec![CapabilityGrant::ResolveDependencies { registries }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> EcosystemManifests {
        EcosystemManifests {
            cargo: true,
            npm: true,
            pypi: true,
            go: true,
        }
    }

    #[test]
    fn every_registry_alias_expands() {
        for (alias, hosts) in REGISTRIES {
            let expanded = expand_registry(alias).expect("alias must expand");
            assert_eq!(expanded.len(), hosts.len(), "{alias} expanded oddly");
        }
    }

    #[test]
    fn a_hostname_the_table_knows_expands_to_itself() {
        assert_eq!(
            expand_registry("crates.io"),
            Some(vec!["crates.io".to_string()])
        );
        // Case and whitespace are normalised, not treated as a new name.
        assert_eq!(
            expand_registry("  Registry.NPMJS.org "),
            Some(vec!["registry.npmjs.org".to_string()])
        );
    }

    #[test]
    fn a_hostname_the_table_does_not_know_justifies_nothing() {
        assert_eq!(expand_registry("evil.example.com"), None);
        assert_eq!(expand_registry(""), None);
        // A near-miss on a known host is still unknown — no suffix matching.
        assert_eq!(expand_registry("registry.npmjs.org.evil.com"), None);
    }

    #[test]
    fn a_read_only_step_gets_nothing_however_rich_the_repo_is() {
        let plan = derive_egress(&all(), false);
        assert!(plan.is_deny());
        assert!(plan.capability_grants.is_empty());
    }

    #[test]
    fn an_empty_repo_gets_nothing_however_writable_the_step_is() {
        let plan = derive_egress(&EcosystemManifests::default(), true);
        assert!(plan.is_deny());
        assert!(plan.capability_grants.is_empty());
    }

    #[test]
    fn a_cargo_repo_gets_crates_and_only_crates() {
        let manifests = EcosystemManifests {
            cargo: true,
            ..Default::default()
        };
        let plan = derive_egress(&manifests, true);

        assert_eq!(plan.granted_registries(), vec!["crates".to_string()]);
        let hosts = plan.network_policy.allowed_hosts();
        assert!(hosts.contains(&"index.crates.io".to_string()));
        assert!(hosts.contains(&"static.crates.io".to_string()));
        assert!(
            !hosts.contains(&"registry.npmjs.org".to_string()),
            "a cargo repo must not reach npm"
        );
    }

    #[test]
    fn a_polyglot_repo_gets_each_ecosystem_once() {
        let plan = derive_egress(&all(), true);
        let hosts = plan.network_policy.allowed_hosts();

        let mut sorted = hosts.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), hosts.len(), "hosts must be deduplicated");

        // Every granted alias is represented, and nothing beyond the table.
        assert_eq!(plan.granted_registries().len(), 4);
        for host in hosts {
            assert!(
                REGISTRIES
                    .iter()
                    .any(|(_, known)| known.contains(&host.as_str())),
                "{host} is not in the registry table"
            );
        }
    }

    #[test]
    fn the_allowlist_is_exactly_what_the_grants_justify() {
        // The property the worker enforces, asserted from the planning side:
        // every host in the allowlist is expanded from a granted alias, so
        // `ungranted_hosts` on the worker has nothing to report.
        let plan = derive_egress(&all(), true);
        let justified: Vec<String> = plan
            .granted_registries()
            .iter()
            .filter_map(|name| expand_registry(name))
            .flatten()
            .collect();

        for host in plan.network_policy.allowed_hosts() {
            assert!(
                justified.contains(host),
                "{host} is allowed but not granted"
            );
        }
    }

    #[test]
    fn the_default_plan_denies() {
        assert!(EgressPlan::default().is_deny());
        assert!(EgressPlan::default().capability_grants.is_empty());
    }

    // --- the provider derivation ---

    const EVERY_PROVIDER: &[ProviderId] = &[
        ProviderId::Claude,
        ProviderId::Openai,
        ProviderId::Gemini,
        ProviderId::Zen,
    ];

    #[test]
    fn every_provider_has_exactly_one_endpoint() {
        for provider in EVERY_PROVIDER {
            let host = provider_host(*provider)
                .unwrap_or_else(|| panic!("{provider:?} has no endpoint in PROVIDER_ENDPOINTS"));
            assert!(!host.is_empty());
            // Round-trips through the grant name, which is what a persisted
            // grant is read back through.
            assert_eq!(expand_provider(provider_grant_name(*provider)), Some(host));
        }
        assert_eq!(PROVIDER_ENDPOINTS.len(), EVERY_PROVIDER.len());
    }

    #[test]
    fn a_routed_step_reaches_that_provider_and_no_other() {
        for provider in EVERY_PROVIDER {
            let plan = derive_provider_egress(*provider);
            let hosts = plan.network_policy.allowed_hosts();

            assert_eq!(hosts.len(), 1, "{provider:?} opened more than one host");
            assert_eq!(hosts[0], provider_host(*provider).expect("has an endpoint"));
            assert_eq!(
                plan.granted_provider(),
                Some(provider_grant_name(*provider))
            );

            // The point of the whole task: no other provider's host is in
            // there. A step routed to one provider cannot reach another.
            for other in EVERY_PROVIDER.iter().filter(|p| *p != provider) {
                let other_host = provider_host(*other).expect("has an endpoint");
                assert!(
                    !hosts.iter().any(|h| h == other_host),
                    "{provider:?} opened {other:?}'s host"
                );
            }
        }
    }

    #[test]
    fn a_provider_name_the_table_does_not_know_justifies_nothing() {
        assert_eq!(expand_provider("evil.example.com"), None);
        assert_eq!(expand_provider(""), None);
        assert_eq!(expand_provider("api.anthropic.com"), None);
        // No suffix matching, same as the registry table.
        assert_eq!(expand_provider("claude.evil.com"), None);
    }

    #[test]
    fn a_provider_grant_names_no_registry_and_a_registry_grant_names_no_provider() {
        // The separation the union must not destroy. Neither derivation can
        // produce the other's grant, so a repository manifest cannot reach a
        // provider and a routing decision cannot open a package registry.
        let provider = derive_provider_egress(ProviderId::Claude);
        assert!(provider.granted_registries().is_empty());

        let ecosystem = derive_egress(&all(), true);
        assert_eq!(ecosystem.granted_provider(), None);
    }

    #[test]
    fn the_union_carries_both_grants_and_both_hosts() {
        let ecosystem = derive_egress(&all(), true);
        let provider = derive_provider_egress(ProviderId::Claude);
        let merged = EgressPlan::union(&ecosystem, &provider);

        assert_eq!(merged.granted_provider(), Some("claude"));
        assert_eq!(merged.granted_registries().len(), 4);

        let hosts = merged.network_policy.allowed_hosts();
        assert!(hosts.contains(&"cortex.heyvera.org".to_string()));
        assert!(hosts.contains(&"registry.npmjs.org".to_string()));

        // Every host still traces to a grant, which is what the worker
        // enforces. A union that widened past its grants would be caught here
        // rather than by an integration test.
        for host in hosts {
            let justified = merged
                .granted_registries()
                .iter()
                .filter_map(|name| expand_registry(name))
                .flatten()
                .any(|granted| granted == *host)
                || merged
                    .granted_provider()
                    .and_then(expand_provider)
                    .is_some_and(|granted| granted == host);
            assert!(justified, "{host} is allowed but not granted");
        }
    }

    #[test]
    fn a_denied_half_does_not_deny_the_other() {
        // A read-only step routed to a provider still reaches the provider:
        // the CLI has to run to read anything. And an unrouted step in a rich
        // repository still gets its registries.
        let read_only = derive_egress(&all(), false);
        let claude = derive_provider_egress(ProviderId::Claude);

        let merged = EgressPlan::union(&read_only, &claude);
        assert_eq!(
            merged.network_policy.allowed_hosts(),
            &["cortex.heyvera.org".to_string()]
        );
        assert_eq!(merged.granted_provider(), Some("claude"));
        assert!(merged.granted_registries().is_empty());

        let merged = EgressPlan::union(&derive_egress(&all(), true), &EgressPlan::deny());
        assert_eq!(merged.granted_provider(), None);
        assert_eq!(merged.granted_registries().len(), 4);
    }

    #[test]
    fn two_denials_are_a_denial() {
        let merged = EgressPlan::union(&EgressPlan::deny(), &EgressPlan::deny());
        assert!(merged.is_deny());
        assert!(merged.capability_grants.is_empty());
    }

    #[test]
    fn the_union_takes_one_provider_not_two() {
        // A caller passing a plan with two provider grants is a bug; the union
        // must not turn that bug into a wider allowlist.
        let two = EgressPlan {
            network_policy: NetworkPolicy::Allowlist {
                hosts: vec![
                    "cortex.heyvera.org".to_string(),
                    "api.openai.com".to_string(),
                ],
            },
            capability_grants: vec![
                CapabilityGrant::ReachProvider {
                    provider: "claude".to_string(),
                },
                CapabilityGrant::ReachProvider {
                    provider: "openai".to_string(),
                },
            ],
        };
        let merged = EgressPlan::union(&EgressPlan::deny(), &two);

        let providers: Vec<_> = merged
            .capability_grants
            .iter()
            .filter(|g| matches!(g, CapabilityGrant::ReachProvider { .. }))
            .collect();
        assert_eq!(providers.len(), 1, "the union kept two provider grants");
        assert_eq!(merged.granted_provider(), Some("claude"));
    }
}
