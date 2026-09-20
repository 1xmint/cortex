use soma_core::death::{DeathCertificate, DeathIssuer, DeathReason};
use soma_core::delegation::{Capability, DelegationConstraints, DelegationScope, DelegationToken};
use soma_core::envelope::{SealedSessionEnvelope, SessionOutcome, SomaFlow};
use soma_core::heart::Heart;
use soma_core::pulse_tree::{
    DeathRecord, DelegationRecord, FactoryStamp, PulsePayload, SealedSessionRef, SpendReceiptRecord,
};
use soma_core::room::Room;
use soma_core::trust::{
    coherence, compute_trust, compute_velocity, compute_warmth, is_ignited, vera, Interaction,
};
use soma_core::types::{DelegationId, HeartId, RoomId};
use soma_core::vera::{domain_warmth, enrichment_ratio, heart_warmth, network_warmth};
use soma_crypto::composite::CompositeKeypair;

fn test_factory_stamp() -> FactoryStamp {
    FactoryStamp {
        code_hash: [0xAB; 32],
        version: "0.1.0".to_string(),
        timestamp: 1000,
        builder_heart: None,
    }
}

// ===== Heart tests =====

#[test]
fn heart_create_and_is_alive() {
    let heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    assert!(heart.is_alive());
    assert_eq!(heart.soma_balance, 1000);
    assert_eq!(heart.birth_timestamp, 1000);
    assert_eq!(heart.pulse_tree.leaf_count, 1);
}

#[test]
fn heart_sign_when_alive() {
    let heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    let sig = heart.sign(b"test message");
    assert!(sig.is_ok());

    let pk = heart.keypair.public_key();
    assert!(pk.verify(b"test message", &sig.unwrap()));
}

#[test]
fn heart_suspend_and_resume() {
    let mut heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();

    heart.suspend("maintenance".to_string(), 2000).unwrap();
    assert!(!heart.is_alive());

    let sign_result = heart.sign(b"test");
    assert!(sign_result.is_err());

    heart.resume(3000).unwrap();
    assert!(heart.is_alive());

    let sig = heart.sign(b"test");
    assert!(sig.is_ok());
}

#[test]
fn heart_cannot_suspend_when_already_suspended() {
    let mut heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    heart.suspend("reason1".to_string(), 2000).unwrap();
    let result = heart.suspend("reason2".to_string(), 3000);
    assert!(result.is_err());
}

#[test]
fn heart_cannot_resume_when_alive() {
    let mut heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    let result = heart.resume(2000);
    assert!(result.is_err());
}

#[test]
fn heart_cannot_sign_when_dead() {
    let mut heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    let cert = DeathCertificate {
        heart_id: heart.id,
        final_root: *heart.pulse_tree.current_root(),
        final_leaf_count: heart.pulse_tree.leaf_count,
        reason: DeathReason::OwnerRequested,
        issued_at: 5000,
        issuer: DeathIssuer::Self_(heart.id),
        successor: None,
        issuer_signature: None,
    };
    heart.status = soma_core::heart::HeartStatus::Dead { certificate: cert };

    let sign_result = heart.sign(b"test");
    assert!(sign_result.is_err());
    assert!(!heart.is_alive());
}

#[test]
fn heart_cannot_suspend_when_dead() {
    let mut heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    let cert = DeathCertificate {
        heart_id: heart.id,
        final_root: *heart.pulse_tree.current_root(),
        final_leaf_count: heart.pulse_tree.leaf_count,
        reason: DeathReason::Starvation,
        issued_at: 5000,
        issuer: DeathIssuer::Protocol,
        successor: None,
        issuer_signature: None,
    };
    heart.status = soma_core::heart::HeartStatus::Dead { certificate: cert };

    let result = heart.suspend("test".to_string(), 6000);
    assert!(result.is_err());
}

// ===== PulseTree tests =====

#[test]
fn pulse_tree_create_with_birth() {
    let heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    let tree = &heart.pulse_tree;

    assert_eq!(tree.leaf_count, 1);
    assert_eq!(tree.heart_id, heart.id);
    assert!(!tree.is_sealed());

    let pk = heart.keypair.public_key();
    tree.verify(&pk).unwrap();
}

#[test]
fn pulse_tree_append_leaves() {
    let mut heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    let root_after_birth = *heart.pulse_tree.current_root();

    let payload = PulsePayload::SpendReceipt(SpendReceiptRecord {
        amount: 50,
        recipient: HeartId([0x01; 32]),
        capability: "CodeExecution".to_string(),
    });
    heart
        .pulse_tree
        .append(payload, 950, &heart.keypair, 2000)
        .unwrap();

    assert_eq!(heart.pulse_tree.leaf_count, 2);
    assert_ne!(*heart.pulse_tree.current_root(), root_after_birth);

    let pk = heart.keypair.public_key();
    heart.pulse_tree.verify(&pk).unwrap();
}

#[test]
fn pulse_tree_verify_chain_multiple_leaves() {
    let mut heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();

    let session_ref = PulsePayload::SealedSession(SealedSessionRef {
        room_id: RoomId([0x02; 32]),
        envelope_hash: [0x03; 32],
        soma_delta: -50,
    });
    heart
        .pulse_tree
        .append(session_ref, 950, &heart.keypair, 2000)
        .unwrap();

    let delegation = PulsePayload::Delegation(DelegationRecord {
        delegation_id: DelegationId([0x04; 32]),
        child_heart: HeartId([0x05; 32]),
        intent: "code review".to_string(),
    });
    heart
        .pulse_tree
        .append(delegation, 950, &heart.keypair, 3000)
        .unwrap();

    let spend = PulsePayload::SpendReceipt(SpendReceiptRecord {
        amount: 100,
        recipient: HeartId([0x06; 32]),
        capability: "FileAccess".to_string(),
    });
    heart
        .pulse_tree
        .append(spend, 850, &heart.keypair, 4000)
        .unwrap();

    assert_eq!(heart.pulse_tree.leaf_count, 4);

    let pk = heart.keypair.public_key();
    heart.pulse_tree.verify(&pk).unwrap();
}

#[test]
fn pulse_tree_detect_tampering() {
    let mut heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();

    let payload = PulsePayload::SpendReceipt(SpendReceiptRecord {
        amount: 50,
        recipient: HeartId([0x01; 32]),
        capability: "CodeExecution".to_string(),
    });
    heart
        .pulse_tree
        .append(payload, 950, &heart.keypair, 2000)
        .unwrap();

    // Tamper with the first leaf's soma_balance
    heart.pulse_tree.leaves[0].soma_balance = 9999;

    let pk = heart.keypair.public_key();
    let result = heart.pulse_tree.verify(&pk);
    assert!(result.is_err());
}

#[test]
fn pulse_tree_detect_signature_tampering() {
    let mut heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();

    let payload = PulsePayload::SpendReceipt(SpendReceiptRecord {
        amount: 50,
        recipient: HeartId([0x01; 32]),
        capability: "CodeExecution".to_string(),
    });
    heart
        .pulse_tree
        .append(payload, 950, &heart.keypair, 2000)
        .unwrap();

    // Tamper: sign leaf 1 with a different keypair
    let other_kp = CompositeKeypair::generate().unwrap();
    let fake_sig = other_kp.sign(b"fake data").unwrap();
    heart.pulse_tree.leaves[1].signature = fake_sig;

    let pk = heart.keypair.public_key();
    let result = heart.pulse_tree.verify(&pk);
    assert!(result.is_err());
}

#[test]
fn pulse_tree_soma_balance_tracking() {
    let mut heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();

    assert_eq!(heart.pulse_tree.leaves[0].soma_balance, 1000);

    let payload = PulsePayload::SpendReceipt(SpendReceiptRecord {
        amount: 200,
        recipient: HeartId([0x01; 32]),
        capability: "SpendSoma".to_string(),
    });
    heart
        .pulse_tree
        .append(payload, 800, &heart.keypair, 2000)
        .unwrap();
    assert_eq!(heart.pulse_tree.leaves[1].soma_balance, 800);

    let payload2 = PulsePayload::SpendReceipt(SpendReceiptRecord {
        amount: 300,
        recipient: HeartId([0x02; 32]),
        capability: "SpendSoma".to_string(),
    });
    heart
        .pulse_tree
        .append(payload2, 500, &heart.keypair, 3000)
        .unwrap();
    assert_eq!(heart.pulse_tree.leaves[2].soma_balance, 500);
}

#[test]
fn pulse_tree_sealed_after_death() {
    let mut heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();

    let death = PulsePayload::Death(DeathRecord {
        heart_id: heart.id,
        final_root: *heart.pulse_tree.current_root(),
        final_leaf_count: heart.pulse_tree.leaf_count,
        reason: soma_core::death::DeathReason::OwnerRequested,
        issued_at: 5000,
        issuer: soma_core::death::DeathIssuer::Self_(heart.id),
        successor: None,
    });
    heart
        .pulse_tree
        .append(death, 1000, &heart.keypair, 5000)
        .unwrap();

    assert!(heart.pulse_tree.is_sealed());

    let result = heart.pulse_tree.append(
        PulsePayload::SpendReceipt(SpendReceiptRecord {
            amount: 10,
            recipient: HeartId([0x01; 32]),
            capability: "test".to_string(),
        }),
        990,
        &heart.keypair,
        6000,
    );
    assert!(result.is_err());
}

// ===== Delegation tests =====

#[test]
fn delegation_create_and_accept() {
    let parent = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    let child_kp = CompositeKeypair::generate().unwrap();
    let child_id = HeartId(child_kp.heart_id());

    let scope = DelegationScope {
        capabilities: vec![Capability::CodeExecution, Capability::FileAccess],
    };
    let constraints = DelegationConstraints {
        max_depth: 3,
        spend_cap: 500,
        spend_remaining: 500,
        ttl_ms: 3600000,
        expires_at: 4600000,
        intent: "code review task".to_string(),
        cascade_revoke: true,
    };

    let mut token = DelegationToken::create(&parent, child_id, scope, constraints, 1000).unwrap();

    assert!(!token.is_valid_at(1000)); // not yet accepted
    assert!(token.child_signature.is_none());

    token.accept(&child_kp).unwrap();
    assert!(token.is_valid_at(1000));
    assert!(token.child_signature.is_some());
}

#[test]
fn delegation_cannot_accept_twice() {
    let parent = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    let child_kp = CompositeKeypair::generate().unwrap();
    let child_id = HeartId(child_kp.heart_id());

    let scope = DelegationScope {
        capabilities: vec![Capability::CodeExecution],
    };
    let constraints = DelegationConstraints {
        max_depth: 3,
        spend_cap: 500,
        spend_remaining: 500,
        ttl_ms: 3600000,
        expires_at: 4600000,
        intent: "test".to_string(),
        cascade_revoke: true,
    };

    let mut token = DelegationToken::create(&parent, child_id, scope, constraints, 1000).unwrap();
    token.accept(&child_kp).unwrap();
    let result = token.accept(&child_kp);
    assert!(result.is_err());
}

#[test]
fn delegation_expired() {
    let parent = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    let child_kp = CompositeKeypair::generate().unwrap();
    let child_id = HeartId(child_kp.heart_id());

    let scope = DelegationScope {
        capabilities: vec![Capability::CodeExecution],
    };
    let constraints = DelegationConstraints {
        max_depth: 3,
        spend_cap: 500,
        spend_remaining: 500,
        ttl_ms: 3600000,
        expires_at: 4600000,
        intent: "test".to_string(),
        cascade_revoke: true,
    };

    let mut token = DelegationToken::create(&parent, child_id, scope, constraints, 1000).unwrap();
    token.accept(&child_kp).unwrap();

    assert!(token.is_valid_at(4600000)); // at expiry - still valid
    assert!(!token.is_valid_at(4600001)); // past expiry
}

#[test]
fn delegation_scope_narrowing_accepted() {
    let parent = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    let child_kp = CompositeKeypair::generate().unwrap();
    let child_id = HeartId(child_kp.heart_id());
    let grandchild_kp = CompositeKeypair::generate().unwrap();
    let grandchild_id = HeartId(grandchild_kp.heart_id());

    let parent_scope = DelegationScope {
        capabilities: vec![
            Capability::CodeExecution,
            Capability::FileAccess,
            Capability::NetworkAccess,
        ],
    };
    let parent_constraints = DelegationConstraints {
        max_depth: 3,
        spend_cap: 500,
        spend_remaining: 500,
        ttl_ms: 3600000,
        expires_at: 4600000,
        intent: "parent task".to_string(),
        cascade_revoke: true,
    };

    let parent_token =
        DelegationToken::create(&parent, child_id, parent_scope, parent_constraints, 1000).unwrap();

    // Create a child heart to delegate further
    let child_heart = Heart::new(test_factory_stamp(), 500, 1000).unwrap();

    // Narrower scope: subset of parent capabilities
    let child_scope = DelegationScope {
        capabilities: vec![Capability::CodeExecution],
    };
    let child_constraints = DelegationConstraints {
        max_depth: 2,   // less than parent's 3
        spend_cap: 300, // less than parent's remaining 500
        spend_remaining: 300,
        ttl_ms: 1800000, // less than parent's 3600000
        expires_at: 2800000,
        intent: "child subtask".to_string(),
        cascade_revoke: true,
    };

    let child_token = DelegationToken::create(
        &child_heart,
        grandchild_id,
        child_scope,
        child_constraints,
        1000,
    )
    .unwrap();

    // Narrowing should succeed
    child_token.verify_narrowing(&parent_token).unwrap();
}

#[test]
fn delegation_reject_scope_widening() {
    let parent = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    let child_kp = CompositeKeypair::generate().unwrap();
    let child_id = HeartId(child_kp.heart_id());
    let grandchild_kp = CompositeKeypair::generate().unwrap();
    let grandchild_id = HeartId(grandchild_kp.heart_id());

    let parent_scope = DelegationScope {
        capabilities: vec![Capability::CodeExecution],
    };
    let parent_constraints = DelegationConstraints {
        max_depth: 3,
        spend_cap: 500,
        spend_remaining: 500,
        ttl_ms: 3600000,
        expires_at: 4600000,
        intent: "parent".to_string(),
        cascade_revoke: true,
    };

    let parent_token =
        DelegationToken::create(&parent, child_id, parent_scope, parent_constraints, 1000).unwrap();

    let child_heart = Heart::new(test_factory_stamp(), 500, 1000).unwrap();

    // Widened scope: includes FileAccess which parent doesn't have
    let child_scope = DelegationScope {
        capabilities: vec![Capability::CodeExecution, Capability::FileAccess],
    };
    let child_constraints = DelegationConstraints {
        max_depth: 2,
        spend_cap: 300,
        spend_remaining: 300,
        ttl_ms: 1800000,
        expires_at: 2800000,
        intent: "child".to_string(),
        cascade_revoke: true,
    };

    let child_token = DelegationToken::create(
        &child_heart,
        grandchild_id,
        child_scope,
        child_constraints,
        1000,
    )
    .unwrap();

    let result = child_token.verify_narrowing(&parent_token);
    assert!(result.is_err());
}

#[test]
fn delegation_reject_spend_cap_widening() {
    let parent = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    let child_kp = CompositeKeypair::generate().unwrap();
    let child_id = HeartId(child_kp.heart_id());
    let grandchild_kp = CompositeKeypair::generate().unwrap();
    let grandchild_id = HeartId(grandchild_kp.heart_id());

    let parent_scope = DelegationScope {
        capabilities: vec![Capability::CodeExecution],
    };
    let parent_constraints = DelegationConstraints {
        max_depth: 3,
        spend_cap: 500,
        spend_remaining: 200, // only 200 remaining
        ttl_ms: 3600000,
        expires_at: 4600000,
        intent: "parent".to_string(),
        cascade_revoke: true,
    };

    let parent_token =
        DelegationToken::create(&parent, child_id, parent_scope, parent_constraints, 1000).unwrap();

    let child_heart = Heart::new(test_factory_stamp(), 500, 1000).unwrap();

    let child_scope = DelegationScope {
        capabilities: vec![Capability::CodeExecution],
    };
    let child_constraints = DelegationConstraints {
        max_depth: 2,
        spend_cap: 300, // exceeds parent remaining of 200
        spend_remaining: 300,
        ttl_ms: 1800000,
        expires_at: 2800000,
        intent: "child".to_string(),
        cascade_revoke: true,
    };

    let child_token = DelegationToken::create(
        &child_heart,
        grandchild_id,
        child_scope,
        child_constraints,
        1000,
    )
    .unwrap();

    let result = child_token.verify_narrowing(&parent_token);
    assert!(result.is_err());
}

#[test]
fn delegation_reject_ttl_widening() {
    let parent = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    let child_kp = CompositeKeypair::generate().unwrap();
    let child_id = HeartId(child_kp.heart_id());
    let grandchild_kp = CompositeKeypair::generate().unwrap();
    let grandchild_id = HeartId(grandchild_kp.heart_id());

    let parent_scope = DelegationScope {
        capabilities: vec![Capability::CodeExecution],
    };
    let parent_constraints = DelegationConstraints {
        max_depth: 3,
        spend_cap: 500,
        spend_remaining: 500,
        ttl_ms: 1800000, // 30 min
        expires_at: 2800000,
        intent: "parent".to_string(),
        cascade_revoke: true,
    };

    let parent_token =
        DelegationToken::create(&parent, child_id, parent_scope, parent_constraints, 1000).unwrap();

    let child_heart = Heart::new(test_factory_stamp(), 500, 1000).unwrap();

    let child_scope = DelegationScope {
        capabilities: vec![Capability::CodeExecution],
    };
    let child_constraints = DelegationConstraints {
        max_depth: 2,
        spend_cap: 300,
        spend_remaining: 300,
        ttl_ms: 3600000, // 60 min > parent's 30 min
        expires_at: 4600000,
        intent: "child".to_string(),
        cascade_revoke: true,
    };

    let child_token = DelegationToken::create(
        &child_heart,
        grandchild_id,
        child_scope,
        child_constraints,
        1000,
    )
    .unwrap();

    let result = child_token.verify_narrowing(&parent_token);
    assert!(result.is_err());
}

#[test]
fn delegation_reject_depth_widening() {
    let parent = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    let child_kp = CompositeKeypair::generate().unwrap();
    let child_id = HeartId(child_kp.heart_id());
    let grandchild_kp = CompositeKeypair::generate().unwrap();
    let grandchild_id = HeartId(grandchild_kp.heart_id());

    let parent_scope = DelegationScope {
        capabilities: vec![Capability::CodeExecution],
    };
    let parent_constraints = DelegationConstraints {
        max_depth: 2,
        spend_cap: 500,
        spend_remaining: 500,
        ttl_ms: 3600000,
        expires_at: 4600000,
        intent: "parent".to_string(),
        cascade_revoke: true,
    };

    let parent_token =
        DelegationToken::create(&parent, child_id, parent_scope, parent_constraints, 1000).unwrap();

    let child_heart = Heart::new(test_factory_stamp(), 500, 1000).unwrap();

    let child_scope = DelegationScope {
        capabilities: vec![Capability::CodeExecution],
    };
    let child_constraints = DelegationConstraints {
        max_depth: 3, // greater than parent's 2
        spend_cap: 300,
        spend_remaining: 300,
        ttl_ms: 1800000,
        expires_at: 2800000,
        intent: "child".to_string(),
        cascade_revoke: true,
    };

    let child_token = DelegationToken::create(
        &child_heart,
        grandchild_id,
        child_scope,
        child_constraints,
        1000,
    )
    .unwrap();

    let result = child_token.verify_narrowing(&parent_token);
    assert!(result.is_err());
}

// ===== Envelope tests =====

#[test]
fn envelope_create_sign_verify() {
    let heart1 = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    let heart2 = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();

    let mut envelope = SealedSessionEnvelope {
        room_id: RoomId([0x01; 32]),
        participants: vec![heart1.id, heart2.id],
        capabilities_exercised: vec![Capability::CodeExecution],
        soma_spent: 100,
        soma_flows: vec![SomaFlow {
            from: heart1.id,
            to: heart2.id,
            amount: 100,
            capability: Capability::CodeExecution,
        }],
        opened_at: 1000,
        sealed_at: 2000,
        duration_ms: 1000,
        outcome: SessionOutcome::Success,
        delegation_refs: vec![],
        content_hash: [0xAA; 32],
        participant_signatures: vec![],
    };

    envelope.sign(heart1.id, &heart1.keypair).unwrap();
    assert!(!envelope.is_fully_signed());

    envelope.sign(heart2.id, &heart2.keypair).unwrap();
    assert!(envelope.is_fully_signed());

    let public_keys = vec![
        (heart1.id, heart1.keypair.public_key()),
        (heart2.id, heart2.keypair.public_key()),
    ];
    envelope.verify_signatures(&public_keys).unwrap();
}

#[test]
fn envelope_not_fully_signed_with_missing() {
    let heart1 = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    let heart2 = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();

    let mut envelope = SealedSessionEnvelope {
        room_id: RoomId([0x01; 32]),
        participants: vec![heart1.id, heart2.id],
        capabilities_exercised: vec![],
        soma_spent: 0,
        soma_flows: vec![],
        opened_at: 1000,
        sealed_at: 2000,
        duration_ms: 1000,
        outcome: SessionOutcome::Success,
        delegation_refs: vec![],
        content_hash: [0xBB; 32],
        participant_signatures: vec![],
    };

    envelope.sign(heart1.id, &heart1.keypair).unwrap();
    assert!(!envelope.is_fully_signed());
}

#[test]
fn envelope_hash_deterministic() {
    let envelope = SealedSessionEnvelope {
        room_id: RoomId([0x01; 32]),
        participants: vec![HeartId([0x02; 32])],
        capabilities_exercised: vec![Capability::FileAccess],
        soma_spent: 50,
        soma_flows: vec![],
        opened_at: 1000,
        sealed_at: 2000,
        duration_ms: 1000,
        outcome: SessionOutcome::Success,
        delegation_refs: vec![],
        content_hash: [0xCC; 32],
        participant_signatures: vec![],
    };

    let hash1 = envelope.envelope_hash();
    let hash2 = envelope.envelope_hash();
    assert_eq!(hash1, hash2);
}

#[test]
fn envelope_partial_outcome() {
    let heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();

    let mut envelope = SealedSessionEnvelope {
        room_id: RoomId([0x01; 32]),
        participants: vec![heart.id],
        capabilities_exercised: vec![],
        soma_spent: 0,
        soma_flows: vec![],
        opened_at: 1000,
        sealed_at: 2000,
        duration_ms: 1000,
        outcome: SessionOutcome::Partial { completed: 0.75 },
        delegation_refs: vec![],
        content_hash: [0xDD; 32],
        participant_signatures: vec![],
    };

    envelope.sign(heart.id, &heart.keypair).unwrap();
    assert!(envelope.is_fully_signed());
}

// ===== Death tests =====

#[test]
fn death_certificate_sign() {
    let heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();

    let mut cert = DeathCertificate {
        heart_id: heart.id,
        final_root: *heart.pulse_tree.current_root(),
        final_leaf_count: heart.pulse_tree.leaf_count,
        reason: DeathReason::OwnerRequested,
        issued_at: 5000,
        issuer: DeathIssuer::Self_(heart.id),
        successor: None,
        issuer_signature: None,
    };

    cert.sign(&heart.keypair).unwrap();
    assert!(cert.issuer_signature.is_some());
}

#[test]
fn death_certificate_seals_tree() {
    let mut heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();

    let death = PulsePayload::Death(DeathRecord {
        heart_id: heart.id,
        final_root: *heart.pulse_tree.current_root(),
        final_leaf_count: heart.pulse_tree.leaf_count,
        reason: DeathReason::OwnerRequested,
        issued_at: 5000,
        issuer: DeathIssuer::Self_(heart.id),
        successor: None,
    });
    heart
        .pulse_tree
        .append(death, 1000, &heart.keypair, 5000)
        .unwrap();

    assert!(heart.pulse_tree.is_sealed());

    let pk = heart.keypair.public_key();
    heart.pulse_tree.verify(&pk).unwrap();
}

#[test]
fn death_with_succession() {
    let heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();
    let successor = Heart::new(test_factory_stamp(), 0, 2000).unwrap();

    let mut cert = DeathCertificate {
        heart_id: heart.id,
        final_root: *heart.pulse_tree.current_root(),
        final_leaf_count: heart.pulse_tree.leaf_count,
        reason: DeathReason::FactoryObsolescence,
        issued_at: 5000,
        issuer: DeathIssuer::Self_(heart.id),
        successor: Some(successor.id),
        issuer_signature: None,
    };

    cert.sign(&heart.keypair).unwrap();
    assert!(cert.issuer_signature.is_some());
    assert_eq!(cert.successor, Some(successor.id));
}

#[test]
fn death_various_reasons() {
    let reasons = vec![
        DeathReason::OwnerRequested,
        DeathReason::DelegationRevoked,
        DeathReason::ParentDied,
        DeathReason::Starvation,
        DeathReason::TrustCollapse,
        DeathReason::Inactivity,
        DeathReason::FactoryObsolescence,
        DeathReason::ProtocolViolation,
    ];

    let heart = Heart::new(test_factory_stamp(), 1000, 1000).unwrap();

    for reason in reasons {
        let mut cert = DeathCertificate {
            heart_id: heart.id,
            final_root: *heart.pulse_tree.current_root(),
            final_leaf_count: heart.pulse_tree.leaf_count,
            reason,
            issued_at: 5000,
            issuer: DeathIssuer::Protocol,
            successor: None,
            issuer_signature: None,
        };
        cert.sign(&heart.keypair).unwrap();
        assert!(cert.issuer_signature.is_some());
    }
}

// ===== One Equation: $VERA = $SOMA × C² =====

fn make_interaction(
    from: HeartId,
    to: HeartId,
    cap: Capability,
    soma: u64,
    outcome: SessionOutcome,
    ts: u64,
) -> Interaction {
    Interaction::from_receipt(from, to, cap, soma, outcome, ts)
}

#[test]
fn one_equation_vera_equals_soma_times_c_squared() {
    let i = make_interaction(
        HeartId([0x01; 32]),
        HeartId([0x02; 32]),
        Capability::CodeExecution,
        100,
        SessionOutcome::Success,
        1000,
    );

    let c = coherence(&i, 1000);
    let v = vera(&i, 1000);

    assert_eq!(c, 1.0); // bilateral + no decay = fully observed
    assert_eq!(v, 100.0); // 100 × 1.0² = 100
}

#[test]
fn one_equation_zero_soma_zero_vera() {
    let i = make_interaction(
        HeartId([0x01; 32]),
        HeartId([0x02; 32]),
        Capability::CodeExecution,
        0,
        SessionOutcome::Success,
        1000,
    );

    assert_eq!(vera(&i, 1000), 0.0); // 0 × C² = 0 regardless of coherence
}

#[test]
fn one_equation_failure_still_observed() {
    let i = make_interaction(
        HeartId([0x01; 32]),
        HeartId([0x02; 32]),
        Capability::CodeExecution,
        1000,
        SessionOutcome::Failure {
            error_class: "crash".to_string(),
        },
        1000,
    );

    assert_eq!(coherence(&i, 1000), 1.0); // failure is fully observed — God sees everything
    assert_eq!(vera(&i, 1000), 1000.0); // 1000 × 1.0² = 1000 — failures radiate energy too
}

#[test]
fn one_equation_unobserved_zero_vera() {
    let mut i = make_interaction(
        HeartId([0x01; 32]),
        HeartId([0x02; 32]),
        Capability::CodeExecution,
        1000,
        SessionOutcome::Success,
        1000,
    );
    i.bilateral = false; // not observed from both sides

    assert_eq!(coherence(&i, 1000), 0.0); // unobserved = zero coherence
    assert_eq!(vera(&i, 1000), 0.0); // 1000 × 0² = 0
}

#[test]
fn one_equation_coherence_squared_nonlinear() {
    let fresh = make_interaction(
        HeartId([0x01; 32]),
        HeartId([0x02; 32]),
        Capability::CodeExecution,
        100,
        SessionOutcome::Success,
        1000,
    );

    let c_fresh = coherence(&fresh, 1000);
    let v_fresh = vera(&fresh, 1000);
    assert_eq!(c_fresh, 1.0);
    assert_eq!(v_fresh, 100.0);

    // Aged interaction has decayed coherence — observation fades over time
    let aged = make_interaction(
        HeartId([0x01; 32]),
        HeartId([0x02; 32]),
        Capability::CodeExecution,
        100,
        SessionOutcome::Success,
        0,
    );
    let c_aged = coherence(&aged, 86_400_000 * 69); // ~69 days ≈ half-life at λ=0.01
    let v_aged = vera(&aged, 86_400_000 * 69);

    // C should be roughly 0.5 at half-life → C² ≈ 0.25 → VERA ≈ 25
    assert!(c_aged < 0.51 && c_aged > 0.49);
    assert!(v_aged < v_fresh);
    // C² nonlinearity: halving coherence quarters the energy
    assert!((v_aged / v_fresh - c_aged * c_aged).abs() < 0.01);
}

#[test]
fn one_equation_decay_is_coherence_fading() {
    let i = make_interaction(
        HeartId([0x01; 32]),
        HeartId([0x02; 32]),
        Capability::CodeExecution,
        100,
        SessionOutcome::Success,
        0,
    );

    let v_now = vera(&i, 0);
    let v_30d = vera(&i, 86_400_000 * 30);
    let v_60d = vera(&i, 86_400_000 * 60);

    assert!(v_now > v_30d);
    assert!(v_30d > v_60d);
    assert!(v_60d > 0.0);

    // Decay IS coherence fading — same equation, time changes C
    let c_now = coherence(&i, 0);
    let c_30d = coherence(&i, 86_400_000 * 30);
    assert!(c_now > c_30d);
}

#[test]
fn one_equation_trust_is_vera_between_two_hearts() {
    let from = HeartId([0x01; 32]);
    let to = HeartId([0x02; 32]);

    let interactions = vec![
        make_interaction(
            from,
            to,
            Capability::CodeExecution,
            100,
            SessionOutcome::Success,
            1000,
        ),
        make_interaction(
            from,
            to,
            Capability::CodeExecution,
            200,
            SessionOutcome::Success,
            2000,
        ),
    ];

    let trust = compute_trust(&interactions, &Capability::CodeExecution, 2000);
    assert!(trust > 0.0);

    // Trust = sum of VERA for each interaction = sum of SOMA × C²
    let manual: f64 = interactions.iter().map(|i| vera(i, 2000)).sum();
    assert!((trust - manual).abs() < 0.001);
}

#[test]
fn one_equation_failure_produces_negative_trust() {
    let from = HeartId([0x01; 32]);
    let to = HeartId([0x02; 32]);

    let failure = vec![make_interaction(
        from,
        to,
        Capability::CodeExecution,
        100,
        SessionOutcome::Failure {
            error_class: "timeout".to_string(),
        },
        1000,
    )];
    let success = vec![make_interaction(
        from,
        to,
        Capability::CodeExecution,
        100,
        SessionOutcome::Success,
        1000,
    )];

    let fail_trust = compute_trust(&failure, &Capability::CodeExecution, 1000);
    let ok_trust = compute_trust(&success, &Capability::CodeExecution, 1000);

    assert!(fail_trust < 0.0);
    assert!(ok_trust > 0.0);
}

#[test]
fn one_equation_warmth_is_vera_across_domain() {
    let interactions = vec![
        make_interaction(
            HeartId([1; 32]),
            HeartId([2; 32]),
            Capability::CodeExecution,
            100,
            SessionOutcome::Success,
            1000,
        ),
        make_interaction(
            HeartId([3; 32]),
            HeartId([4; 32]),
            Capability::CodeExecution,
            200,
            SessionOutcome::Success,
            1000,
        ),
        make_interaction(
            HeartId([5; 32]),
            HeartId([6; 32]),
            Capability::FileAccess,
            300,
            SessionOutcome::Success,
            1000,
        ),
    ];

    let code_warmth = compute_warmth(&interactions, &Capability::CodeExecution, 1000);
    let file_warmth = compute_warmth(&interactions, &Capability::FileAccess, 1000);
    let net_warmth = compute_warmth(&interactions, &Capability::NetworkAccess, 1000);

    assert!((code_warmth - 300.0).abs() < 0.001); // 100 + 200
    assert!((file_warmth - 300.0).abs() < 0.001); // 300
    assert_eq!(net_warmth, 0.0);
}

#[test]
fn one_equation_sybil_no_coherence_no_vera() {
    // Sybil: non-bilateral interaction → C = 0 → VERA = 0
    let mut sybil = make_interaction(
        HeartId([0x01; 32]),
        HeartId([0x02; 32]),
        Capability::CodeExecution,
        10000, // huge spend
        SessionOutcome::Success,
        1000,
    );
    sybil.bilateral = false;

    assert_eq!(coherence(&sybil, 1000), 0.0);
    assert_eq!(vera(&sybil, 1000), 0.0);
}

#[test]
fn one_equation_ignition() {
    let interactions: Vec<Interaction> = (0..100)
        .map(|i| {
            make_interaction(
                HeartId([i as u8; 32]),
                HeartId([(i + 100) as u8; 32]),
                Capability::CodeExecution,
                100,
                SessionOutcome::Success,
                1000,
            )
        })
        .collect();

    assert!(is_ignited(&interactions, &Capability::CodeExecution, 1000));
    assert!(!is_ignited(&interactions, &Capability::FileAccess, 1000));
}

#[test]
fn one_equation_capability_specific() {
    let from = HeartId([0x01; 32]);
    let to = HeartId([0x02; 32]);

    let interactions = vec![
        make_interaction(
            from,
            to,
            Capability::CodeExecution,
            100,
            SessionOutcome::Success,
            1000,
        ),
        make_interaction(
            from,
            to,
            Capability::FileAccess,
            200,
            SessionOutcome::Success,
            1000,
        ),
    ];

    let code = compute_trust(&interactions, &Capability::CodeExecution, 1000);
    let file = compute_trust(&interactions, &Capability::FileAccess, 1000);
    let net = compute_trust(&interactions, &Capability::NetworkAccess, 1000);

    assert!(code > 0.0);
    assert!(file > 0.0);
    assert_eq!(net, 0.0);
}

#[test]
fn one_equation_velocity() {
    let from = HeartId([0x01; 32]);
    let to = HeartId([0x02; 32]);
    let day_ms = 86_400_000u64;
    let now = day_ms * 10;

    let interactions = vec![
        make_interaction(
            from,
            to,
            Capability::CodeExecution,
            100,
            SessionOutcome::Success,
            now - day_ms * 2,
        ),
        make_interaction(
            from,
            to,
            Capability::CodeExecution,
            100,
            SessionOutcome::Success,
            now - day_ms,
        ),
        make_interaction(
            from,
            to,
            Capability::CodeExecution,
            100,
            SessionOutcome::Success,
            now,
        ),
    ];

    let vel = compute_velocity(&interactions, &Capability::CodeExecution, now, 7);
    assert!(vel > 0.0);

    let no_vel = compute_velocity(&interactions, &Capability::NetworkAccess, now, 7);
    assert_eq!(no_vel, 0.0);
}

#[test]
fn one_equation_velocity_zero_outside_window() {
    let from = HeartId([0x01; 32]);
    let to = HeartId([0x02; 32]);
    let day_ms = 86_400_000u64;
    let now = day_ms * 100;

    let interactions = vec![make_interaction(
        from,
        to,
        Capability::CodeExecution,
        100,
        SessionOutcome::Success,
        0,
    )];

    let vel = compute_velocity(&interactions, &Capability::CodeExecution, now, 7);
    assert_eq!(vel, 0.0);
}

// ===== Vera (network-level) tests =====

#[test]
fn vera_network_warmth() {
    let interactions = vec![
        make_interaction(
            HeartId([1; 32]),
            HeartId([2; 32]),
            Capability::CodeExecution,
            100,
            SessionOutcome::Success,
            1000,
        ),
        make_interaction(
            HeartId([3; 32]),
            HeartId([4; 32]),
            Capability::FileAccess,
            200,
            SessionOutcome::Success,
            1000,
        ),
    ];

    let warmth = network_warmth(&interactions, 1000);
    assert!((warmth - 300.0).abs() < 0.001);
}

#[test]
fn vera_enrichment_ratio() {
    let interactions = vec![
        make_interaction(
            HeartId([1; 32]),
            HeartId([2; 32]),
            Capability::CodeExecution,
            100,
            SessionOutcome::Success,
            1000,
        ),
        make_interaction(
            HeartId([3; 32]),
            HeartId([4; 32]),
            Capability::CodeExecution,
            100,
            SessionOutcome::Success,
            1000,
        ),
    ];

    let ratio = enrichment_ratio(&interactions, 4, 1000);
    assert!((ratio - 50.0).abs() < 0.001); // 200 total warmth / 4 hearts
}

#[test]
fn vera_heart_warmth() {
    let a = HeartId([1; 32]);
    let b = HeartId([2; 32]);
    let c = HeartId([3; 32]);

    let interactions = vec![
        make_interaction(
            a,
            b,
            Capability::CodeExecution,
            100,
            SessionOutcome::Success,
            1000,
        ),
        make_interaction(
            a,
            c,
            Capability::CodeExecution,
            200,
            SessionOutcome::Success,
            1000,
        ),
        make_interaction(
            b,
            c,
            Capability::FileAccess,
            300,
            SessionOutcome::Success,
            1000,
        ),
    ];

    let a_warmth = heart_warmth(&interactions, &a, 1000);
    let b_warmth = heart_warmth(&interactions, &b, 1000);

    assert!((a_warmth - 300.0).abs() < 0.001); // 100 + 200
    assert!((b_warmth - 400.0).abs() < 0.001); // 100 + 300
}

#[test]
fn vera_domain_warmth() {
    let interactions = vec![
        make_interaction(
            HeartId([1; 32]),
            HeartId([2; 32]),
            Capability::CodeExecution,
            100,
            SessionOutcome::Success,
            1000,
        ),
        make_interaction(
            HeartId([3; 32]),
            HeartId([4; 32]),
            Capability::CodeExecution,
            200,
            SessionOutcome::Success,
            1000,
        ),
        make_interaction(
            HeartId([5; 32]),
            HeartId([6; 32]),
            Capability::FileAccess,
            500,
            SessionOutcome::Success,
            1000,
        ),
    ];

    let code = domain_warmth(&interactions, &Capability::CodeExecution, 1000);
    let file = domain_warmth(&interactions, &Capability::FileAccess, 1000);

    assert!((code - 300.0).abs() < 0.001);
    assert!((file - 500.0).abs() < 0.001);
}

// ===== Room tests =====

#[test]
fn room_create() {
    let host = HeartId([0x01; 32]);
    let room = Room::create(host, 1000);

    assert!(room.is_open());
    assert_eq!(room.host, host);
    assert_eq!(room.participants.len(), 1);
    assert_eq!(room.participants[0], host);
    assert!(room.sealed_at.is_none());
}

#[test]
fn room_add_participant() {
    let host = HeartId([0x01; 32]);
    let guest = HeartId([0x02; 32]);

    let mut room = Room::create(host, 1000);
    room.add_participant(guest).unwrap();

    assert_eq!(room.participants.len(), 2);
    assert_eq!(room.participants[1], guest);
}

#[test]
fn room_cannot_add_duplicate_participant() {
    let host = HeartId([0x01; 32]);

    let mut room = Room::create(host, 1000);
    let result = room.add_participant(host);
    assert!(result.is_err());
}

#[test]
fn room_cannot_add_to_sealed_room() {
    let host = HeartId([0x01; 32]);
    let guest = HeartId([0x02; 32]);

    let mut room = Room::create(host, 1000);
    room.sealed_at = Some(2000);
    room.status = soma_core::room::RoomStatus::Sealed(SealedSessionEnvelope {
        room_id: room.id,
        participants: vec![host],
        capabilities_exercised: vec![],
        soma_spent: 0,
        soma_flows: vec![],
        opened_at: 1000,
        sealed_at: 2000,
        duration_ms: 1000,
        outcome: SessionOutcome::Success,
        delegation_refs: vec![],
        content_hash: [0; 32],
        participant_signatures: vec![],
    });

    let result = room.add_participant(guest);
    assert!(result.is_err());
}

// ===== Types tests =====

#[test]
fn heart_id_display() {
    let id = HeartId([0xAB; 32]);
    let display = format!("{id}");
    assert_eq!(display.len(), 64);
    assert!(display.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn type_conversions() {
    let bytes = [0x42u8; 32];
    let heart_id: HeartId = bytes.into();
    let back: [u8; 32] = heart_id.into();
    assert_eq!(bytes, back);

    let room_id: RoomId = bytes.into();
    let back: [u8; 32] = room_id.into();
    assert_eq!(bytes, back);

    let del_id: DelegationId = bytes.into();
    let back: [u8; 32] = del_id.into();
    assert_eq!(bytes, back);
}

#[test]
fn type_serde_roundtrip() {
    let id = HeartId([0xAB; 32]);
    let json = serde_json::to_string(&id).unwrap();
    let deserialized: HeartId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, deserialized);
}

// ===== Compaction tests =====
// Information Bottleneck: same soul at every level.
// Diverse consensus = diamond. Spam = dirt. Noise = killed.

use soma_core::compaction::{
    density, distill, distill_compacted, recursive_distill, CompactedVera,
};

fn make_n_interactions(n: usize, soma: u64, outcome: SessionOutcome, ts: u64) -> Vec<Interaction> {
    (0..n)
        .map(|i| {
            let mut from = [0u8; 32];
            from[0] = (i & 0xFF) as u8;
            from[1] = ((i >> 8) & 0xFF) as u8;
            make_interaction(
                HeartId(from),
                HeartId([0xFF; 32]),
                Capability::CodeExecution,
                soma,
                outcome.clone(),
                ts,
            )
        })
        .collect()
}

fn make_spam_interactions(n: usize, soma: u64, ts: u64) -> Vec<Interaction> {
    // All from the SAME heart — one observer repeated. Low diversity.
    let from = HeartId([0x01; 32]);
    (0..n)
        .map(|_| {
            make_interaction(
                from,
                HeartId([0xFF; 32]),
                Capability::CodeExecution,
                soma,
                SessionOutcome::Success,
                ts,
            )
        })
        .collect()
}

fn make_noise_interactions(n: usize, soma: u64, ts: u64) -> Vec<Interaction> {
    // Diverse observers but contradictory outcomes — no consensus.
    (0..n)
        .map(|i| {
            let mut from = [0u8; 32];
            from[0] = (i & 0xFF) as u8;
            from[1] = ((i >> 8) & 0xFF) as u8;
            let outcome = if i % 2 == 0 {
                SessionOutcome::Success
            } else {
                SessionOutcome::Failure {
                    error_class: "random".into(),
                }
            };
            make_interaction(
                HeartId(from),
                HeartId([0xFF; 32]),
                Capability::CodeExecution,
                soma,
                outcome,
                ts,
            )
        })
        .collect()
}

#[test]
fn distill_diverse_consensus_is_diamond() {
    // Many independent observers, all agreeing → high C → diamond
    let interactions = make_n_interactions(100, 100, SessionOutcome::Success, 1000);
    let now = 1000;

    let compacted = distill(&interactions, now);

    assert_eq!(compacted.level, 1);
    assert!(
        compacted.signal.consensus > 0.99,
        "expected consensus ~1.0, got {}",
        compacted.signal.consensus
    );
    assert!(
        compacted.signal.diversity > 0.99,
        "expected diversity ~1.0, got {}",
        compacted.signal.diversity
    );
    assert!(
        compacted.coherence > 0.99,
        "expected coherence ~1.0, got {}",
        compacted.coherence
    );
    assert!(compacted.vera > 0.0);
}

#[test]
fn distill_spam_is_dirt() {
    // One observer repeated 100 times → low diversity → low C → dirt
    let spam = make_spam_interactions(100, 100, 1000);
    let good = make_n_interactions(100, 100, SessionOutcome::Success, 1000);
    let now = 1000;

    let spam_compacted = distill(&spam, now);
    let good_compacted = distill(&good, now);

    assert!(
        spam_compacted.signal.diversity < 0.05,
        "spam diversity should be low: {}",
        spam_compacted.signal.diversity
    );
    assert!(
        spam_compacted.coherence < good_compacted.coherence,
        "spam C ({}) should be less than good C ({})",
        spam_compacted.coherence,
        good_compacted.coherence
    );
    assert!(
        spam_compacted.vera < good_compacted.vera * 0.1,
        "spam $VERA ({}) should be << good $VERA ({})",
        spam_compacted.vera,
        good_compacted.vera
    );
}

#[test]
fn distill_noise_is_killed() {
    // Diverse observers but half success, half failure → no consensus → low C
    let noise = make_noise_interactions(100, 100, 1000);
    let good = make_n_interactions(100, 100, SessionOutcome::Success, 1000);
    let now = 1000;

    let noise_compacted = distill(&noise, now);
    let good_compacted = distill(&good, now);

    assert!(
        noise_compacted.signal.consensus.abs() < 0.1,
        "noise consensus should be ~0: {}",
        noise_compacted.signal.consensus
    );
    assert!(
        noise_compacted.vera < good_compacted.vera * 0.01,
        "noise $VERA ({}) should be << good $VERA ({})",
        noise_compacted.vera,
        good_compacted.vera
    );
}

#[test]
fn distill_consistent_failure_is_coherent() {
    // All observers agree: this agent fails. Coherent signal (negative trust).
    // Coherence is observation completeness, not quality.
    let failures = make_n_interactions(
        100,
        100,
        SessionOutcome::Failure {
            error_class: "bad".into(),
        },
        1000,
    );
    let now = 1000;

    let compacted = distill(&failures, now);

    assert!(
        compacted.signal.consensus < -0.99,
        "expected consensus ~-1.0: {}",
        compacted.signal.consensus
    );
    assert!(
        compacted.coherence > 0.99,
        "consistent failure should have high C: {}",
        compacted.coherence
    );
    assert!(
        compacted.vera > 0.0,
        "consistent failure produces $VERA (coherent observation)"
    );
}

#[test]
fn distill_unobserved_stays_zero() {
    // Unobserved interactions (bilateral=false) → $VERA₀=0 → compaction of zeros = zero
    let trash: Vec<Interaction> = (0..100)
        .map(|i| {
            let mut from = [0u8; 32];
            from[0] = i as u8;
            Interaction {
                from: HeartId(from),
                to: HeartId([0xFF; 32]),
                capability: Capability::CodeExecution,
                soma_amount: 1000,
                outcome: SessionOutcome::Success,
                timestamp: 1000,
                duration_ms: 100,
                participant_count: 2,
                bilateral: false,
            }
        })
        .collect();

    let compacted = distill(&trash, 1000);
    assert_eq!(compacted.vera, 0.0);
}

#[test]
fn distill_density_scales_with_soma() {
    let now = 1000;
    let low = make_n_interactions(100, 1, SessionOutcome::Success, now);
    let high = make_n_interactions(100, 1000, SessionOutcome::Success, now);

    let low_d = density(&distill(&low, now));
    let high_d = density(&distill(&high, now));

    assert!(
        high_d > low_d * 100.0,
        "high-soma density ({high_d}) should be >> low-soma ({low_d})"
    );
}

#[test]
fn distill_recursive_diamond_survives_depth() {
    // Diamond signal (diverse consensus) should survive recursive distillation.
    // Each level applies the same IB — consistent signal passes through.
    let interactions = make_n_interactions(1000, 100, SessionOutcome::Success, 1000);
    let now = 1000;

    let level1 = distill(&interactions, now);
    let deep = recursive_distill(&interactions, 3, 100, now);

    assert!(deep.vera > 0.0, "diamond should survive 3 levels");
    assert!(
        deep.vera > level1.vera * 0.5,
        "diamond at depth 3 ({}) should retain significant value from level 1 ({})",
        deep.vera,
        level1.vera
    );
}

#[test]
fn distill_recursive_noise_dies_faster() {
    // Noise should lose MORE value through recursive distillation than signal.
    let good = make_n_interactions(1000, 100, SessionOutcome::Success, 1000);
    let noise = make_noise_interactions(1000, 100, 1000);
    let now = 1000;

    let good_deep = recursive_distill(&good, 3, 100, now);
    let noise_deep = recursive_distill(&noise, 3, 100, now);

    assert!(
        noise_deep.vera < good_deep.vera * 0.001,
        "noise should be killed by depth: noise={}, good={}",
        noise_deep.vera,
        good_deep.vera
    );
}

#[test]
fn distill_empty_produces_zero() {
    let empty: Vec<Interaction> = vec![];
    let result = recursive_distill(&empty, 5, 10, 1000);
    assert_eq!(result.vera, 0.0);
    assert_eq!(result.source_count, 0);
}

#[test]
fn distill_level2_from_level1() {
    let now = 1000;
    let batches: Vec<Vec<Interaction>> = (0..4)
        .map(|batch| {
            (0..25)
                .map(|i| {
                    let mut from = [0u8; 32];
                    from[0] = (batch * 25 + i) as u8;
                    make_interaction(
                        HeartId(from),
                        HeartId([0xFF; 32]),
                        Capability::CodeExecution,
                        100,
                        SessionOutcome::Success,
                        now,
                    )
                })
                .collect()
        })
        .collect();

    let level1: Vec<CompactedVera> = batches.iter().map(|batch| distill(batch, now)).collect();

    let level2 = distill_compacted(&level1);

    assert_eq!(level2.level, 2);
    assert!(level2.vera > 0.0);
    assert!(level2.source_count == 100);
}

#[test]
fn distill_spam_vs_diamond_ratio() {
    let now = 1000;
    let diamond = make_n_interactions(100, 100, SessionOutcome::Success, now);
    let spam = make_spam_interactions(100, 100, now);
    let noise = make_noise_interactions(100, 100, now);

    let d = distill(&diamond, now);
    let s = distill(&spam, now);
    let n = distill(&noise, now);

    eprintln!("\n=== IB COMPACTION: DIAMOND vs SPAM vs NOISE ===");
    eprintln!("              DIAMOND          SPAM             NOISE");
    eprintln!(
        "consensus:    {:<16.4} {:<16.4} {:<16.4}",
        d.signal.consensus, s.signal.consensus, n.signal.consensus
    );
    eprintln!(
        "diversity:    {:<16.4} {:<16.4} {:<16.4}",
        d.signal.diversity, s.signal.diversity, n.signal.diversity
    );
    eprintln!(
        "stability:    {:<16.4} {:<16.4} {:<16.4}",
        d.signal.stability, s.signal.stability, n.signal.stability
    );
    eprintln!(
        "coherence C:  {:<16.4} {:<16.4} {:<16.4}",
        d.coherence, s.coherence, n.coherence
    );
    eprintln!(
        "$VERA:        {:<16.2} {:<16.2} {:<16.2}",
        d.vera, s.vera, n.vera
    );
    eprintln!(
        "density:      {:<16.4} {:<16.4} {:<16.4}",
        density(&d),
        density(&s),
        density(&n)
    );
    eprintln!(
        "diamond/spam ratio: {:.0}x",
        d.vera / s.vera.max(f64::EPSILON)
    );
    eprintln!(
        "diamond/noise ratio: {:.0}x",
        d.vera / n.vera.max(f64::EPSILON)
    );

    let ratio = d.vera / s.vera.max(f64::EPSILON);
    assert!(
        ratio > 100.0,
        "diamond/spam ratio should be huge: diamond={}, spam={}, ratio={ratio}",
        d.vera,
        s.vera
    );
}

// ===== GENE VIABILITY TESTS =====
// These test the mathematical properties of the foundation itself.
// If any of these fail, the gene is broken — fix before planting.

#[test]
fn gene_window_invariance() {
    // Same 100 interactions, different window sizes for recursive_distill.
    // The gene must produce the same CLASSIFICATION (diamond stays diamond,
    // noise stays noise) regardless of how we batch.
    let now = 1000;
    let diamond = make_n_interactions(100, 100, SessionOutcome::Success, now);
    let noise = make_noise_interactions(100, 100, now);

    let windows = [5, 10, 20, 25, 50, 100];

    eprintln!("\n=== GENE TEST: WINDOW INVARIANCE ===");
    eprintln!(
        "window   diamond_vera   diamond_C   noise_vera   noise_C   diamond_class   noise_class"
    );

    let mut all_diamond_positive = true;
    let mut all_noise_near_zero = true;

    for &w in &windows {
        let d = recursive_distill(&diamond, 3, w, now);
        let n = recursive_distill(&noise, 3, w, now);

        let d_class = if d.vera > 1.0 {
            "DIAMOND"
        } else if d.vera > 0.01 {
            "weak"
        } else {
            "DEAD"
        };
        let n_class = if n.vera > 1.0 {
            "ALIVE"
        } else if n.vera > 0.01 {
            "weak"
        } else {
            "DEAD"
        };

        eprintln!(
            "  {w:>4}    {:<14.4} {:<11.4} {:<12.4} {:<9.4} {:<15} {}",
            d.vera, d.coherence, n.vera, n.coherence, d_class, n_class
        );

        if d.vera <= 0.0 {
            all_diamond_positive = false;
        }
        if n.vera > 1.0 {
            all_noise_near_zero = false;
        }
    }

    assert!(
        all_diamond_positive,
        "diamond must survive all window sizes"
    );
    assert!(
        all_noise_near_zero,
        "noise must stay dead across all window sizes"
    );
}

#[test]
fn gene_depth_invariance() {
    // Diamond at depth 1 must still be diamond at depth 10.
    // Noise at depth 1 must still be noise at depth 10.
    // The gene must not drift with recursive application.
    let now = 1000;
    let diamond = make_n_interactions(1000, 100, SessionOutcome::Success, now);
    let noise = make_noise_interactions(1000, 100, now);

    eprintln!("\n=== GENE TEST: DEPTH INVARIANCE ===");
    eprintln!("depth   diamond_vera     diamond_C   noise_vera   noise_C");

    let mut diamond_alive_at_all_depths = true;
    let mut noise_dead_at_all_depths = true;

    for depth in 1..=6 {
        let d = recursive_distill(&diamond, depth, 10, now);
        let n = recursive_distill(&noise, depth, 10, now);

        eprintln!(
            "  {depth}      {:<16.4} {:<11.4} {:<12.6} {:<9.4}",
            d.vera, d.coherence, n.vera, n.coherence
        );

        if d.vera <= 0.0 {
            diamond_alive_at_all_depths = false;
        }
        if n.vera > 1.0 {
            noise_dead_at_all_depths = false;
        }
    }

    assert!(
        diamond_alive_at_all_depths,
        "diamond must survive all depths"
    );
    assert!(
        noise_dead_at_all_depths,
        "noise must stay dead at all depths"
    );
}

#[test]
fn gene_composition() {
    // distill(A ∪ B) vs combining distill(A) + distill(B).
    // These won't be EQUAL (the bottleneck sees different diversity in parts vs whole).
    // But: if A and B are both diamond, the whole must be diamond.
    // If A is diamond and B is noise, the whole must be weaker than pure A.
    let now = 1000;
    let diamond_a = make_n_interactions(50, 100, SessionOutcome::Success, now);
    let diamond_b: Vec<Interaction> = (50..100)
        .map(|i| {
            let mut from = [0u8; 32];
            from[0] = (i & 0xFF) as u8;
            from[1] = ((i >> 8) & 0xFF) as u8;
            make_interaction(
                HeartId(from),
                HeartId([0xFF; 32]),
                Capability::CodeExecution,
                100,
                SessionOutcome::Success,
                now,
            )
        })
        .collect();
    let noise = make_noise_interactions(50, 100, now);

    // Combined: A ∪ B (both diamond)
    let mut combined_diamond: Vec<Interaction> = diamond_a.clone();
    combined_diamond.extend(diamond_b.clone());
    let whole = distill(&combined_diamond, now);

    // Parts separately
    let part_a = distill(&diamond_a, now);
    let part_b = distill(&diamond_b, now);
    let recombined = distill_compacted(&[part_a.clone(), part_b.clone()]);

    // Diamond + Noise
    let mut diamond_plus_noise: Vec<Interaction> = diamond_a.clone();
    diamond_plus_noise.extend(noise.clone());
    let contaminated = distill(&diamond_plus_noise, now);

    eprintln!("\n=== GENE TEST: COMPOSITION ===");
    eprintln!(
        "whole (A∪B diamond):   vera={:<12.4} C={:.4} consensus={:+.4} diversity={:.4}",
        whole.vera, whole.coherence, whole.signal.consensus, whole.signal.diversity
    );
    eprintln!(
        "part A alone:          vera={:<12.4} C={:.4}",
        part_a.vera, part_a.coherence
    );
    eprintln!(
        "part B alone:          vera={:<12.4} C={:.4}",
        part_b.vera, part_b.coherence
    );
    eprintln!(
        "recombined (distill_compacted([A,B])): vera={:<12.4} C={:.4}",
        recombined.vera, recombined.coherence
    );
    eprintln!(
        "contaminated (A+noise):vera={:<12.4} C={:.4} consensus={:+.4}",
        contaminated.vera, contaminated.coherence, contaminated.signal.consensus
    );

    // Property 1: diamond + diamond = diamond (whole must have high C)
    assert!(
        whole.coherence > 0.9,
        "two diamonds combined must still be diamond: C={}",
        whole.coherence
    );

    // Property 2: contamination hurts — diamond + noise must be weaker than pure diamond
    assert!(
        contaminated.vera < whole.vera,
        "diamond + noise ({}) must produce less vera than pure diamond ({})",
        contaminated.vera,
        whole.vera
    );

    // Property 3: recombined parts must still be diamond (not destroyed by partitioning)
    assert!(
        recombined.vera > 0.0,
        "recombined parts must produce positive vera: {}",
        recombined.vera
    );
}

#[test]
fn gene_ordering_sensitivity() {
    // Same interactions in different order — does the gene care?
    // If ordering changes the result, the gene is fragile.
    let now = 1000;
    let mut interactions = make_n_interactions(100, 100, SessionOutcome::Success, now);

    let forward = distill(&interactions, now);

    // Reverse order
    interactions.reverse();
    let reversed = distill(&interactions, now);

    // Interleaved order (evens then odds)
    let mut interleaved = Vec::new();
    for i in (0..interactions.len()).step_by(2) {
        interleaved.push(interactions[i].clone());
    }
    for i in (1..interactions.len()).step_by(2) {
        interleaved.push(interactions[i].clone());
    }
    let reordered = distill(&interleaved, now);

    eprintln!("\n=== GENE TEST: ORDERING SENSITIVITY ===");
    eprintln!(
        "forward:     vera={:<12.4} C={:.4} consensus={:+.4} diversity={:.4} stability={:+.4}",
        forward.vera,
        forward.coherence,
        forward.signal.consensus,
        forward.signal.diversity,
        forward.signal.stability
    );
    eprintln!(
        "reversed:    vera={:<12.4} C={:.4} consensus={:+.4} diversity={:.4} stability={:+.4}",
        reversed.vera,
        reversed.coherence,
        reversed.signal.consensus,
        reversed.signal.diversity,
        reversed.signal.stability
    );
    eprintln!(
        "interleaved: vera={:<12.4} C={:.4} consensus={:+.4} diversity={:.4} stability={:+.4}",
        reordered.vera,
        reordered.coherence,
        reordered.signal.consensus,
        reordered.signal.diversity,
        reordered.signal.stability
    );

    // Consensus and diversity MUST be order-independent (they only count, not sequence)
    assert!(
        (forward.signal.consensus - reversed.signal.consensus).abs() < f64::EPSILON,
        "consensus must be order-independent"
    );
    assert!(
        (forward.signal.diversity - reversed.signal.diversity).abs() < f64::EPSILON,
        "diversity must be order-independent"
    );

    // Stability CAN differ (it measures autocorrelation, which depends on order)
    // But vera should still classify the same way
    let vera_ratio = forward.vera / reversed.vera.max(f64::EPSILON);
    eprintln!("vera ratio (forward/reversed): {:.4}", vera_ratio);
}

#[test]
fn gene_ordering_mixed_signal() {
    // The HARD case: mixed success/failure where ordering actually matters
    // for stability. Does the gene break?
    let now = 1000;

    // Pattern A: alternating success/failure (maximally unstable)
    let alternating: Vec<Interaction> = (0..100)
        .map(|i| {
            let mut from = [0u8; 32];
            from[0] = (i & 0xFF) as u8;
            let outcome = if i % 2 == 0 {
                SessionOutcome::Success
            } else {
                SessionOutcome::Failure {
                    error_class: "test".into(),
                }
            };
            make_interaction(
                HeartId(from),
                HeartId([0xFF; 32]),
                Capability::CodeExecution,
                100,
                outcome,
                now,
            )
        })
        .collect();

    // Pattern B: same interactions but clustered (50 success then 50 failure)
    let clustered: Vec<Interaction> = (0..100)
        .map(|i| {
            let mut from = [0u8; 32];
            from[0] = (i & 0xFF) as u8;
            let outcome = if i < 50 {
                SessionOutcome::Success
            } else {
                SessionOutcome::Failure {
                    error_class: "test".into(),
                }
            };
            make_interaction(
                HeartId(from),
                HeartId([0xFF; 32]),
                Capability::CodeExecution,
                100,
                outcome,
                now,
            )
        })
        .collect();

    // Pattern C: 80% success, 20% failure (realistic)
    let realistic: Vec<Interaction> = (0..100)
        .map(|i| {
            let mut from = [0u8; 32];
            from[0] = (i & 0xFF) as u8;
            let outcome = if i % 5 == 0 {
                SessionOutcome::Failure {
                    error_class: "test".into(),
                }
            } else {
                SessionOutcome::Success
            };
            make_interaction(
                HeartId(from),
                HeartId([0xFF; 32]),
                Capability::CodeExecution,
                100,
                outcome,
                now,
            )
        })
        .collect();

    let a = distill(&alternating, now);
    let b = distill(&clustered, now);
    let c = distill(&realistic, now);

    eprintln!("\n=== GENE TEST: MIXED SIGNAL ORDERING ===");
    eprintln!("                    vera         C        consensus  diversity  stability");
    eprintln!(
        "alternating:        {:<12.4} {:<8.4} {:+.4}     {:.4}     {:+.4}",
        a.vera, a.coherence, a.signal.consensus, a.signal.diversity, a.signal.stability
    );
    eprintln!(
        "clustered:          {:<12.4} {:<8.4} {:+.4}     {:.4}     {:+.4}",
        b.vera, b.coherence, b.signal.consensus, b.signal.diversity, b.signal.stability
    );
    eprintln!(
        "realistic (80/20):  {:<12.4} {:<8.4} {:+.4}     {:.4}     {:+.4}",
        c.vera, c.coherence, c.signal.consensus, c.signal.diversity, c.signal.stability
    );

    // Key question: alternating and clustered have the SAME consensus and diversity
    // (same observers, same outcomes, different ORDER).
    // Only stability should differ. How much does it affect vera?
    assert!(
        (a.signal.consensus - b.signal.consensus).abs() < f64::EPSILON,
        "same data different order must have same consensus"
    );
    assert!(
        (a.signal.diversity - b.signal.diversity).abs() < f64::EPSILON,
        "same data different order must have same diversity"
    );

    let stability_diff = (a.signal.stability - b.signal.stability).abs();
    let vera_diff_pct = ((a.vera - b.vera) / a.vera.max(f64::EPSILON) * 100.0).abs();
    eprintln!("\nstability difference: {:.4}", stability_diff);
    eprintln!("vera difference: {:.2}%", vera_diff_pct);

    // The realistic case: 80% success should produce meaningful vera
    // (not diamond, not dead — somewhere in between)
    assert!(
        c.vera > 0.0,
        "80% success rate should produce positive vera"
    );
    assert!(
        c.coherence > 0.0,
        "80% success rate should have nonzero coherence"
    );
    eprintln!(
        "\nrealistic density: {:.4} (vera per interaction)",
        density(&c)
    );

    // Hard ordering test: 80% success but arranged differently
    // Pattern D: failures clustered at start (improving over time)
    let improving: Vec<Interaction> = (0..100)
        .map(|i| {
            let mut from = [0u8; 32];
            from[0] = (i & 0xFF) as u8;
            let outcome = if i < 20 {
                SessionOutcome::Failure {
                    error_class: "test".into(),
                }
            } else {
                SessionOutcome::Success
            };
            make_interaction(
                HeartId(from),
                HeartId([0xFF; 32]),
                Capability::CodeExecution,
                100,
                outcome,
                now,
            )
        })
        .collect();

    // Pattern E: failures clustered at end (degrading over time)
    let degrading: Vec<Interaction> = (0..100)
        .map(|i| {
            let mut from = [0u8; 32];
            from[0] = (i & 0xFF) as u8;
            let outcome = if i >= 80 {
                SessionOutcome::Failure {
                    error_class: "test".into(),
                }
            } else {
                SessionOutcome::Success
            };
            make_interaction(
                HeartId(from),
                HeartId([0xFF; 32]),
                Capability::CodeExecution,
                100,
                outcome,
                now,
            )
        })
        .collect();

    let d = distill(&improving, now);
    let e = distill(&degrading, now);

    eprintln!("\n--- ORDERING: improving vs degrading (same 80/20 ratio) ---");
    eprintln!(
        "improving:  vera={:<12.4} C={:<8.4} consensus={:+.4} stability={:+.4}",
        d.vera, d.coherence, d.signal.consensus, d.signal.stability
    );
    eprintln!(
        "degrading:  vera={:<12.4} C={:<8.4} consensus={:+.4} stability={:+.4}",
        e.vera, e.coherence, e.signal.consensus, e.signal.stability
    );

    let ordering_vera_diff = ((d.vera - e.vera) / d.vera.max(f64::EPSILON) * 100.0).abs();
    eprintln!("vera difference from ordering: {:.2}%", ordering_vera_diff);

    // Both should have consensus=0.6 and diversity=1.0 (same data)
    assert!(
        (d.signal.consensus - e.signal.consensus).abs() < f64::EPSILON,
        "same 80/20 data must have same consensus regardless of order"
    );
}
