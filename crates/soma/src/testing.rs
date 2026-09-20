use std::collections::HashMap;
use std::path::Path;

use crate::crypto::{generate_keypair, random_bytes};
use crate::delegation::{create_delegation, Caveat, Delegation, InvocationContext};
use crate::did::public_key_to_did;
use crate::identity::HeartIdentity;
use crate::SomaError;

/// Test utilities for Soma identity and delegation testing
pub struct SomaTestKit {
    /// Generated test identities with their roles
    pub identities: HashMap<String, TestIdentity>,
    /// Active delegations for testing
    pub delegations: HashMap<String, Delegation>,
}

#[derive(Debug, Clone)]
pub struct TestIdentity {
    pub identity: HeartIdentity,
    pub role: String,
    pub authority_level: String,
    pub clearance_level: String,
}

impl SomaTestKit {
    /// Create a new test kit with predefined organizational identities
    pub fn new() -> Result<Self, SomaError> {
        let mut kit = Self {
            identities: HashMap::new(),
            delegations: HashMap::new(),
        };

        // Create organizational hierarchy
        kit.create_identity("ceo", "PolicyMaker", "TopSecret", "ceo-v1")?;
        kit.create_identity("security-admin", "PolicyMaker", "TopSecret", "security-v1")?;
        kit.create_identity("architect", "Architect", "Confidential", "arch-v1")?;
        kit.create_identity("team-lead", "TeamLead", "Internal", "lead-v1")?;
        kit.create_identity("senior-dev", "Individual", "Internal", "senior-dev-v1")?;
        kit.create_identity("developer", "Individual", "Public", "dev-v1")?;
        kit.create_identity("intern", "Individual", "Public", "intern-v1")?;

        // Create external entities
        kit.create_identity("external-auditor", "Individual", "Restricted", "auditor-v1")?;
        kit.create_identity("compliance-officer", "PolicyMaker", "Restricted", "compliance-v1")?;

        // Create delegation chains
        kit.setup_delegation_chains()?;

        Ok(kit)
    }

    /// Create a test identity with specific role and clearance
    pub fn create_identity(
        &mut self,
        name: &str,
        authority_level: &str,
        clearance_level: &str,
        runtime_id: &str,
    ) -> Result<(), SomaError> {
        let identity = HeartIdentity::new("heyvera", "cortex-test", runtime_id)?;

        let test_identity = TestIdentity {
            identity,
            role: name.to_string(),
            authority_level: authority_level.to_string(),
            clearance_level: clearance_level.to_string(),
        };

        self.identities.insert(name.to_string(), test_identity);
        Ok(())
    }

    /// Get test identity by name
    pub fn get_identity(&self, name: &str) -> Option<&TestIdentity> {
        self.identities.get(name)
    }

    /// Create standard organizational delegation chains
    fn setup_delegation_chains(&mut self) -> Result<(), SomaError> {
        // CEO -> Security Admin delegation (full access)
        self.create_delegation(
            "ceo",
            "security-admin",
            vec!["*".to_string()],
            vec![
                Caveat::Budget { credits: 1000000.0 },
                Caveat::Capabilities { allow: vec!["*".to_string()] },
            ],
            "ceo-to-security",
        )?;

        // CEO -> Architect delegation (technical decisions)
        self.create_delegation(
            "ceo",
            "architect",
            vec!["memory:*".to_string(), "system:*".to_string()],
            vec![
                Caveat::Budget { credits: 100000.0 },
                Caveat::Capabilities {
                    allow: vec![
                        "memory:read".to_string(),
                        "memory:write".to_string(),
                        "memory:share".to_string(),
                        "system:configure".to_string()
                    ]
                },
            ],
            "ceo-to-architect",
        )?;

        // Architect -> Team Lead delegation (team scope)
        self.create_delegation(
            "architect",
            "team-lead",
            vec!["memory:read".to_string(), "memory:write".to_string()],
            vec![
                Caveat::Budget { credits: 10000.0 },
                Caveat::Capabilities {
                    allow: vec![
                        "memory:read".to_string(),
                        "memory:write".to_string(),
                    ]
                },
                Caveat::MaxInvocations { count: 1000 },
            ],
            "architect-to-teamlead",
        )?;

        // Team Lead -> Senior Developer delegation (limited scope)
        self.create_delegation(
            "team-lead",
            "senior-dev",
            vec!["memory:read".to_string()],
            vec![
                Caveat::Budget { credits: 1000.0 },
                Caveat::Capabilities {
                    allow: vec!["memory:read".to_string()]
                },
                Caveat::MaxInvocations { count: 500 },
            ],
            "teamlead-to-seniordev",
        )?;

        // Team Lead -> Developer delegation (read-only)
        self.create_delegation(
            "team-lead",
            "developer",
            vec!["memory:read".to_string()],
            vec![
                Caveat::Budget { credits: 500.0 },
                Caveat::Capabilities {
                    allow: vec!["memory:read".to_string()]
                },
                Caveat::MaxInvocations { count: 100 },
            ],
            "teamlead-to-dev",
        )?;

        // Special: External auditor delegation (read-only, time-limited)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        self.create_delegation(
            "compliance-officer",
            "external-auditor",
            vec!["memory:read".to_string()],
            vec![
                Caveat::Budget { credits: 100.0 },
                Caveat::Capabilities {
                    allow: vec!["memory:read".to_string()]
                },
                Caveat::ExpiresAt { timestamp: now + (24 * 60 * 60 * 1000) }, // 24 hours
                Caveat::Audience { did: self.get_identity("external-auditor").unwrap().identity.did.clone() },
            ],
            "compliance-to-auditor",
        )?;

        Ok(())
    }

    /// Create a delegation between two identities
    pub fn create_delegation(
        &mut self,
        issuer_name: &str,
        subject_name: &str,
        capabilities: Vec<String>,
        caveats: Vec<Caveat>,
        delegation_name: &str,
    ) -> Result<(), SomaError> {
        let issuer = self.identities.get(issuer_name)
            .ok_or_else(|| SomaError::Delegation(format!("Unknown issuer: {}", issuer_name)))?;

        let subject = self.identities.get(subject_name)
            .ok_or_else(|| SomaError::Delegation(format!("Unknown subject: {}", subject_name)))?;

        let delegation = create_delegation(
            &issuer.identity.secret_key,
            &issuer.identity.public_key,
            &issuer.identity.did,
            &subject.identity.did,
            capabilities,
            caveats,
            None, // No parent for now - could be enhanced for chain testing
        )?;

        self.delegations.insert(delegation_name.to_string(), delegation);
        Ok(())
    }

    /// Get delegation by name
    pub fn get_delegation(&self, name: &str) -> Option<&Delegation> {
        self.delegations.get(name)
    }

    /// Create invocation context for testing delegation verification
    pub fn create_invocation_context(
        &self,
        invoker_name: &str,
        capability: &str,
        audience_name: Option<&str>,
        credits_spent: Option<f64>,
        invocation_count: Option<u64>,
    ) -> Option<InvocationContext> {
        let invoker = self.get_identity(invoker_name)?;

        Some(InvocationContext {
            invoker_did: invoker.identity.did.clone(),
            audience_did: audience_name.and_then(|name|
                self.get_identity(name).map(|id| id.identity.did.clone())
            ),
            capability: capability.to_string(),
            credits_spent,
            cumulative_credits_spent: credits_spent,
            invocation_count,
            now: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64
            ),
            host: Some("localhost".to_string()),
        })
    }

    /// Test delegation verification scenarios
    pub fn test_delegation_scenarios(&self) -> Vec<DelegationTestResult> {
        let mut results = Vec::new();

        // Test 1: Valid CEO -> Security Admin delegation
        let ctx = self.create_invocation_context("security-admin", "*", None, Some(500.0), Some(5))
            .expect("Should create context");

        let delegation = self.get_delegation("ceo-to-security")
            .expect("Should have CEO->Security delegation");

        let result = crate::delegation::verify_delegation(delegation, &ctx);
        results.push(DelegationTestResult {
            test_name: "CEO -> Security Admin (full access)".to_string(),
            delegation_name: "ceo-to-security".to_string(),
            expected: true,
            actual: result.as_ref().map_or(false, |v| v.is_valid()),
            error: result.err().map(|e| e.to_string()),
        });

        // Test 2: Valid Team Lead -> Developer delegation (read-only)
        let ctx = self.create_invocation_context("developer", "memory:read", None, Some(50.0), Some(10))
            .expect("Should create context");

        let delegation = self.get_delegation("teamlead-to-dev")
            .expect("Should have TeamLead->Dev delegation");

        let result = crate::delegation::verify_delegation(delegation, &ctx);
        results.push(DelegationTestResult {
            test_name: "Team Lead -> Developer (read access)".to_string(),
            delegation_name: "teamlead-to-dev".to_string(),
            expected: true,
            actual: result.as_ref().map_or(false, |v| v.is_valid()),
            error: result.err().map(|e| e.to_string()),
        });

        // Test 3: Invalid capability (developer trying to write)
        let ctx = self.create_invocation_context("developer", "memory:write", None, Some(50.0), Some(10))
            .expect("Should create context");

        let delegation = self.get_delegation("teamlead-to-dev")
            .expect("Should have TeamLead->Dev delegation");

        let result = crate::delegation::verify_delegation(delegation, &ctx);
        results.push(DelegationTestResult {
            test_name: "Developer attempting write access".to_string(),
            delegation_name: "teamlead-to-dev".to_string(),
            expected: false,
            actual: result.as_ref().map_or(false, |v| v.is_valid()),
            error: result.err().map(|e| e.to_string()),
        });

        // Test 4: Budget exceeded
        let ctx = self.create_invocation_context("developer", "memory:read", None, Some(5000.0), Some(10))
            .expect("Should create context");

        let delegation = self.get_delegation("teamlead-to-dev")
            .expect("Should have TeamLead->Dev delegation");

        let result = crate::delegation::verify_delegation(delegation, &ctx);
        results.push(DelegationTestResult {
            test_name: "Developer exceeding budget limit".to_string(),
            delegation_name: "teamlead-to-dev".to_string(),
            expected: false,
            actual: result.as_ref().map_or(false, |v| v.is_valid()),
            error: result.err().map(|e| e.to_string()),
        });

        // Test 5: Time-limited external auditor (should be valid)
        let ctx = self.create_invocation_context("external-auditor", "memory:read",
            Some("external-auditor"), Some(50.0), Some(5))
            .expect("Should create context");

        let delegation = self.get_delegation("compliance-to-auditor")
            .expect("Should have Compliance->Auditor delegation");

        let result = crate::delegation::verify_delegation(delegation, &ctx);
        results.push(DelegationTestResult {
            test_name: "External auditor read access".to_string(),
            delegation_name: "compliance-to-auditor".to_string(),
            expected: true,
            actual: result.as_ref().map_or(false, |v| v.is_valid()),
            error: result.err().map(|e| e.to_string()),
        });

        results
    }

    /// Generate test DIDs for capability system integration
    pub fn generate_test_dids(count: usize) -> Vec<TestDID> {
        (0..count).map(|i| {
            let (sk, pk) = generate_keypair();
            let did = public_key_to_did(&pk);

            TestDID {
                name: format!("test-did-{:03}", i),
                did,
                secret_key: sk,
                public_key: pk,
            }
        }).collect()
    }

    /// Save all test identities to files for debugging
    pub fn save_test_identities(&self, base_path: &Path) -> Result<(), SomaError> {
        std::fs::create_dir_all(base_path)
            .map_err(|e| SomaError::Serialization(format!("Failed to create directory: {}", e)))?;

        for (name, test_identity) in &self.identities {
            let file_path = base_path.join(format!("{}.json", name));
            test_identity.identity.save(&file_path)?;
        }

        // Save delegation summary
        let delegations_summary = self.delegations.iter()
            .map(|(name, delegation)| {
                serde_json::json!({
                    "name": name,
                    "id": delegation.id,
                    "issuer": delegation.issuer_did,
                    "subject": delegation.subject_did,
                    "capabilities": delegation.capabilities,
                    "caveats_count": delegation.caveats.len(),
                })
            })
            .collect::<Vec<_>>();

        let summary_path = base_path.join("delegations_summary.json");
        let summary_json = serde_json::to_string_pretty(&delegations_summary)
            .map_err(|e| SomaError::Serialization(e.to_string()))?;

        std::fs::write(summary_path, summary_json)
            .map_err(|e| SomaError::Serialization(format!("Failed to write summary: {}", e)))?;

        Ok(())
    }

    /// Get organizational chart as JSON for visualization
    pub fn get_org_chart(&self) -> serde_json::Value {
        serde_json::json!({
            "identities": self.identities.iter().map(|(name, identity)| {
                serde_json::json!({
                    "name": name,
                    "did": identity.identity.did,
                    "authority_level": identity.authority_level,
                    "clearance_level": identity.clearance_level,
                    "role": identity.role
                })
            }).collect::<Vec<_>>(),
            "delegations": self.delegations.iter().map(|(name, delegation)| {
                serde_json::json!({
                    "name": name,
                    "issuer_did": delegation.issuer_did,
                    "subject_did": delegation.subject_did,
                    "capabilities": delegation.capabilities,
                    "budget": delegation.caveats.iter()
                        .find_map(|c| match c {
                            Caveat::Budget { credits } => Some(credits),
                            _ => None
                        })
                })
            }).collect::<Vec<_>>()
        })
    }
}

#[derive(Debug, Clone)]
pub struct TestDID {
    pub name: String,
    pub did: String,
    pub secret_key: Vec<u8>,
    pub public_key: Vec<u8>,
}

#[derive(Debug)]
pub struct DelegationTestResult {
    pub test_name: String,
    pub delegation_name: String,
    pub expected: bool,
    pub actual: bool,
    pub error: Option<String>,
}

impl DelegationTestResult {
    pub fn is_success(&self) -> bool {
        self.expected == self.actual
    }
}

/// Integration helper for Capability system testing
pub struct SomaCapabilityIntegration;

impl SomaCapabilityIntegration {
    /// Convert Soma identity to Capability system mock identity
    pub fn soma_to_capability_identity(
        soma_identity: &TestIdentity,
    ) -> crate::MockIdentity {
        // This would be implemented to bridge Soma and Capability systems
        // For now, create a mock that maps Soma concepts to Capability concepts

        let authority_level = match soma_identity.authority_level.as_str() {
            "PolicyMaker" => crate::AuthorityLevel::PolicyMaker,
            "Architect" => crate::AuthorityLevel::Architect,
            "TeamLead" => crate::AuthorityLevel::TeamLead,
            _ => crate::AuthorityLevel::Individual,
        };

        let clearance = match soma_identity.clearance_level.as_str() {
            "TopSecret" => crate::ClassificationLevel::TopSecret,
            "Restricted" => crate::ClassificationLevel::Restricted,
            "Confidential" => crate::ClassificationLevel::Confidential,
            "Internal" => crate::ClassificationLevel::Internal,
            _ => crate::ClassificationLevel::Public,
        };

        crate::MockIdentity {
            did: soma_identity.identity.did.clone(),
            authority_level: Some(authority_level),
            security_clearance: Some(clearance),
            active_delegation: None, // Would be populated from Soma delegation
        }
    }

    /// Create Capability constraint from Soma delegation
    pub fn delegation_to_constraints(
        delegation: &Delegation,
    ) -> Vec<crate::CapabilityConstraint> {
        let mut constraints = Vec::new();

        for caveat in &delegation.caveats {
            match caveat {
                Caveat::ExpiresAt { timestamp } => {
                    let expires_at = chrono::DateTime::from_timestamp(*timestamp as i64 / 1000, 0)
                        .unwrap_or_else(|| chrono::Utc::now());
                    constraints.push(crate::CapabilityConstraint::ValidBetween {
                        start: chrono::Utc::now(),
                        end: expires_at,
                    });
                }
                Caveat::Capabilities { allow } => {
                    constraints.push(crate::CapabilityConstraint::RequiresSomaCapabilities(
                        allow.clone()
                    ));
                }
                Caveat::Budget { credits: _ } => {
                    // Could map to usage constraints
                }
                Caveat::Audience { did } => {
                    // Could map to workspace or user constraints
                }
                _ => {} // Other caveats don't directly map
            }
        }

        constraints
    }
}

// Mock types for integration (would reference actual Capability types in real implementation)
mod crate {
    #[derive(Debug, Clone)]
    pub struct MockIdentity {
        pub did: String,
        pub authority_level: Option<AuthorityLevel>,
        pub security_clearance: Option<ClassificationLevel>,
        pub active_delegation: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq)]
    pub enum AuthorityLevel {
        Individual,
        TeamLead,
        Architect,
        PolicyMaker,
    }

    #[derive(Debug, Clone, PartialEq)]
    pub enum ClassificationLevel {
        Public,
        Internal,
        Confidential,
        Restricted,
        TopSecret,
    }

    #[derive(Debug, Clone)]
    pub enum CapabilityConstraint {
        RequiresSomaCapabilities(Vec<String>),
        ValidBetween {
            start: chrono::DateTime<chrono::Utc>,
            end: chrono::DateTime<chrono::Utc>,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_soma_test_kit_creation() {
        let kit = SomaTestKit::new().expect("Should create test kit");

        // Verify all standard identities were created
        assert!(kit.get_identity("ceo").is_some());
        assert!(kit.get_identity("security-admin").is_some());
        assert!(kit.get_identity("architect").is_some());
        assert!(kit.get_identity("team-lead").is_some());
        assert!(kit.get_identity("developer").is_some());
        assert!(kit.get_identity("external-auditor").is_some());

        // Verify delegations were created
        assert!(kit.get_delegation("ceo-to-security").is_some());
        assert!(kit.get_delegation("teamlead-to-dev").is_some());
        assert!(kit.get_delegation("compliance-to-auditor").is_some());

        println!("✅ Soma test kit created with {} identities and {} delegations",
            kit.identities.len(), kit.delegations.len());
    }

    #[test]
    fn test_delegation_verification_scenarios() {
        let kit = SomaTestKit::new().expect("Should create test kit");
        let results = kit.test_delegation_scenarios();

        println!("=== Delegation Verification Test Results ===");

        let mut passed = 0;
        let mut total = 0;

        for result in &results {
            total += 1;
            if result.is_success() {
                passed += 1;
                println!("✅ {}: PASS", result.test_name);
            } else {
                println!("❌ {}: FAIL (expected: {}, actual: {})",
                    result.test_name, result.expected, result.actual);
                if let Some(error) = &result.error {
                    println!("   Error: {}", error);
                }
            }
        }

        println!("Summary: {}/{} tests passed", passed, total);

        // At least basic delegations should work
        assert!(passed > 0, "At least some delegation tests should pass");
    }

    #[test]
    fn test_did_generation() {
        let dids = SomaTestKit::generate_test_dids(5);
        assert_eq!(dids.len(), 5);

        for (i, did) in dids.iter().enumerate() {
            assert!(did.did.starts_with("did:key:z"));
            assert_eq!(did.secret_key.len(), 32);
            assert_eq!(did.public_key.len(), 32);
            assert_eq!(did.name, format!("test-did-{:03}", i));
        }

        println!("✅ Generated {} test DIDs", dids.len());
        for did in &dids {
            println!("   {}: {}", did.name, did.did);
        }
    }

    #[test]
    fn test_organizational_hierarchy() {
        let kit = SomaTestKit::new().expect("Should create test kit");

        // Test organizational structure
        let ceo = kit.get_identity("ceo").unwrap();
        let architect = kit.get_identity("architect").unwrap();
        let developer = kit.get_identity("developer").unwrap();

        assert_eq!(ceo.authority_level, "PolicyMaker");
        assert_eq!(ceo.clearance_level, "TopSecret");

        assert_eq!(architect.authority_level, "Architect");
        assert_eq!(architect.clearance_level, "Confidential");

        assert_eq!(developer.authority_level, "Individual");
        assert_eq!(developer.clearance_level, "Public");

        // Verify DIDs are unique
        let all_dids: std::collections::HashSet<_> = kit.identities
            .values()
            .map(|id| &id.identity.did)
            .collect();
        assert_eq!(all_dids.len(), kit.identities.len(), "All DIDs should be unique");

        println!("✅ Organizational hierarchy validated");
        println!("   CEO: {} ({})", ceo.identity.did, ceo.authority_level);
        println!("   Architect: {} ({})", architect.identity.did, architect.authority_level);
        println!("   Developer: {} ({})", developer.identity.did, developer.authority_level);
    }
}