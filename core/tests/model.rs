//! Model profiles: which model a role uses, and what its calls cost.

use cortex::model::{EngineKind, Roles, builtin, estimate_usd_micros, find, load_profiles};
use cortex::payload::Tokens;

const SHIPPED: [&str; 4] = ["fake", "claude-cli-sonnet", "claude-cli-opus", "api-sonnet"];

#[test]
fn embedded_profiles_parse_and_ids_are_unique() {
    let home = tempfile::tempdir().unwrap();
    let profiles = load_profiles(home.path()).unwrap();
    let mut ids: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
    for id in SHIPPED {
        assert!(ids.contains(&id), "{id} missing: {ids:?}");
    }
    let count = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), count, "duplicate ids");

    let sonnet = find(&profiles, "claude-cli-sonnet").unwrap();
    assert_eq!(sonnet.engine, EngineKind::ClaudeCli);
    assert_eq!(sonnet.model, "claude-sonnet-5-5");
    assert!(sonnet.provisional());
    let opus = find(&profiles, "claude-cli-opus").unwrap();
    assert_eq!(opus.model, "claude-opus-5-5");
    let api = find(&profiles, "api-sonnet").unwrap();
    assert_eq!(api.engine, EngineKind::Api);
    assert_eq!(api.model, "claude-sonnet-5-5");
    assert!(!api.provisional());
    assert_eq!(find(&profiles, "fake").unwrap().engine, EngineKind::Fake);
    // A price is only shipped with the page it was copied from.
    for p in &profiles {
        if p.price_in_per_mtok_usd.is_some() {
            assert!(p.price_source.is_some(), "{} has no price_source", p.id);
        }
    }
    assert_eq!(Roles::default().worker, "claude-cli-sonnet");
    assert!(find(&profiles, &Roles::default().worker).is_ok());
}

#[test]
fn home_profile_overrides_by_id() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("profiles.toml"),
        r#"
[[profile]]
id = "claude-cli-sonnet"
engine = "claude-cli"
model = "claude-sonnet-5-5"
price_in_per_mtok_usd = 9.0
price_out_per_mtok_usd = 99.0

[[profile]]
id = "my-api-opus"
engine = "api"
model = "claude-opus-5-5"
"#,
    )
    .unwrap();
    let profiles = load_profiles(home.path()).unwrap();
    let sonnet = find(&profiles, "claude-cli-sonnet").unwrap();
    assert_eq!(sonnet.price_in_per_mtok_usd, Some(9.0));
    assert_eq!(sonnet.price_out_per_mtok_usd, Some(99.0));
    // The home entry replaces the whole profile, not single fields.
    assert_eq!(sonnet.price_cache_read_per_mtok_usd, None);
    assert_eq!(
        find(&profiles, "claude-cli-opus").unwrap(),
        builtin("claude-cli-opus").unwrap()
    );
    assert_eq!(
        find(&profiles, "my-api-opus").unwrap().engine,
        EngineKind::Api
    );
    let shipped = load_profiles(tempfile::tempdir().unwrap().path()).unwrap();
    assert_eq!(profiles.len(), shipped.len() + 1);
}

#[test]
fn home_profiles_with_duplicate_ids_are_rejected() {
    let home = tempfile::tempdir().unwrap();
    let entry = "[[profile]]\nid = \"x\"\nengine = \"api\"\nmodel = \"m\"\n";
    std::fs::write(home.path().join("profiles.toml"), format!("{entry}{entry}")).unwrap();
    let err = load_profiles(home.path()).unwrap_err().to_string();
    assert!(err.contains("x"), "{err}");
}

#[test]
fn unknown_profile_names_the_known_ones() {
    let profiles = load_profiles(tempfile::tempdir().unwrap().path()).unwrap();
    let err = find(&profiles, "nope").unwrap_err().to_string();
    assert!(
        err.contains("nope") && err.contains("claude-cli-sonnet"),
        "{err}"
    );
}

#[test]
fn missing_price_estimates_none() {
    let tokens = Tokens {
        input: Some(1000),
        output: Some(10),
        ..Tokens::default()
    };
    let mut p = builtin("api-sonnet").unwrap();
    assert!(estimate_usd_micros(&p, &tokens).is_some());
    p.price_out_per_mtok_usd = None;
    assert_eq!(estimate_usd_micros(&p, &tokens), None);

    // A missing token count is unknown, never zero.
    let p = builtin("api-sonnet").unwrap();
    let no_output = Tokens {
        output: None,
        ..tokens.clone()
    };
    assert_eq!(estimate_usd_micros(&p, &no_output), None);

    // Cache tokens with no cache price cannot be costed.
    let mut p = builtin("api-sonnet").unwrap();
    p.price_cache_read_per_mtok_usd = None;
    let cached = Tokens {
        cache_read: Some(5),
        ..tokens.clone()
    };
    assert_eq!(estimate_usd_micros(&p, &cached), None);
    // ...but no cache tokens need no cache price.
    assert!(estimate_usd_micros(&p, &tokens).is_some());

    assert_eq!(
        estimate_usd_micros(&builtin("fake").unwrap(), &tokens),
        None
    );
}

#[test]
fn estimate_is_exact_for_known_prices() {
    let mut p = builtin("api-sonnet").unwrap();
    p.price_in_per_mtok_usd = Some(3.0);
    p.price_out_per_mtok_usd = Some(15.0);
    let tokens = Tokens {
        input: Some(1_000_000),
        output: Some(0),
        ..Tokens::default()
    };
    assert_eq!(estimate_usd_micros(&p, &tokens), Some(3_000_000));

    // The shipped Sonnet prices: $2 in, $10 out, $0.10 cache read and
    // $2.50 cache write per million tokens.
    let p = builtin("api-sonnet").unwrap();
    let tokens = Tokens {
        input: Some(1_000_000),
        output: Some(100_000),
        cache_read: Some(1_000_000),
        cache_write: Some(1_000_000),
    };
    assert_eq!(
        estimate_usd_micros(&p, &tokens),
        Some(2_000_000 + 1_000_000 + 100_000 + 2_500_000)
    );
}

#[test]
fn a_profile_serves_its_model_and_dated_versions_of_it() {
    let p = builtin("claude-cli-sonnet").unwrap();
    assert!(p.serves("claude-sonnet-5-5"));
    assert!(p.serves("claude-sonnet-5-5-20261001"));
    assert!(!p.serves("claude-sonnet-5-5-preview"));
    assert!(!p.serves("claude-opus-5-5"));
    assert!(!builtin("claude-cli-opus").unwrap().serves("claude-opus-5"));
}
