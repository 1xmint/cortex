use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use cortex_api::state::AppState;

const ROUTE_MANIFEST: &str = include_str!("../route-manifest.csv");
const ROUTER_SOURCE: &str = include_str!("../src/lib.rs");
const CADDYFILE: &str = include_str!("../../../Caddyfile");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Owner {
    Cortex,
    Duplicate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Contract {
    owner: Owner,
    methods: BTreeSet<String>,
    path: String,
}

fn contracts() -> Vec<Contract> {
    ROUTE_MANIFEST
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut columns = line.splitn(3, ',');
            let owner = match columns.next().expect("owner") {
                "cortex" => Owner::Cortex,
                "duplicate" => Owner::Duplicate,
                unknown => panic!("unexpected route owner {unknown}: only cortex-standalone owners (cortex, duplicate) belong in this manifest"),
            };
            let methods = columns
                .next()
                .expect("methods")
                .split('|')
                .map(str::to_string)
                .collect();
            let path = columns.next().expect("path").to_string();
            Contract {
                owner,
                methods,
                path,
            }
        })
        .collect()
}

fn function_slice<'a>(source: &'a str, name: &str) -> &'a str {
    let start_marker = format!("pub fn {name}");
    let start = source.find(&start_marker).expect("router function exists");
    &source[start..]
}

fn routes_in(source: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut routes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let bytes = source.as_bytes();
    let mut cursor = 0;

    while let Some(relative_start) = source[cursor..].find(".route(") {
        let start = cursor + relative_start;
        let mut index = start + ".route(".len();
        let mut depth = 1_u32;
        let mut in_string = false;
        let mut escaped = false;

        while index < bytes.len() && depth > 0 {
            let byte = bytes[index];
            if in_string {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    in_string = false;
                }
            } else if byte == b'"' {
                in_string = true;
            } else if byte == b'(' {
                depth += 1;
            } else if byte == b')' {
                depth -= 1;
            }
            index += 1;
        }

        assert_eq!(depth, 0, "unbalanced .route call");
        let call = &source[start..index];
        let quote_start = call.find('"').expect("route path starts with a quote") + 1;
        let quote_end = call[quote_start..]
            .find('"')
            .map(|offset| quote_start + offset)
            .expect("route path ends with a quote");
        let path = call[quote_start..quote_end].to_string();
        let methods = routes.entry(path).or_default();
        for method in ["DELETE", "GET", "PATCH", "POST", "PUT"] {
            if call.contains(&format!("{}(", method.to_ascii_lowercase())) {
                methods.insert(method.to_string());
            }
        }
        assert!(!methods.is_empty(), "route has no recognized HTTP method");
        cursor = index;
    }

    routes
}

fn expected_routes(owners: &[Owner]) -> BTreeMap<String, BTreeSet<String>> {
    contracts()
        .into_iter()
        .filter(|contract| owners.contains(&contract.owner))
        .map(|contract| (contract.path, contract.methods))
        .collect()
}

fn caddy_site(label: &str) -> &str {
    let start = CADDYFILE.find(label).expect("Caddy site exists");
    let opening = CADDYFILE[start..]
        .find('{')
        .map(|offset| start + offset)
        .expect("Caddy site opens");
    let mut depth = 0_i32;
    for (offset, byte) in CADDYFILE.as_bytes()[opening..].iter().enumerate() {
        if *byte == b'{' {
            depth += 1;
        } else if *byte == b'}' {
            depth -= 1;
            if depth == 0 {
                return &CADDYFILE[start..=opening + offset];
            }
        }
    }
    panic!("Caddy site block is unbalanced")
}

async fn send(app: axum::Router, method: &str, path: &str) -> axum::response::Response {
    app.oneshot(
        Request::builder()
            .method(Method::from_str(method).expect("valid method"))
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .expect("request"),
    )
    .await
    .expect("response")
}

async fn response_json(response: axum::response::Response) -> serde_json::Value {
    let body = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&body).expect("JSON response")
}

#[test]
fn build_heyvera_router_no_longer_exists() {
    assert!(
        !ROUTER_SOURCE.contains("fn build_heyvera_router"),
        "cortex-standalone must not carry the Socials router builder"
    );
}

#[test]
fn literal_manifest_exactly_matches_cortex_router() {
    let manifest = contracts();
    assert_eq!(
        manifest.len(),
        106,
        "cortex-standalone keeps 91 cortex + 15 duplicate routes"
    );
    assert_eq!(
        manifest
            .iter()
            .map(|contract| contract.path.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        106,
        "every path template must have exactly one owner"
    );
    assert_eq!(
        manifest
            .iter()
            .filter(|contract| contract.owner == Owner::Cortex)
            .count(),
        91
    );
    assert_eq!(
        manifest
            .iter()
            .filter(|contract| contract.owner == Owner::Duplicate)
            .count(),
        15
    );

    let cortex_source = function_slice(ROUTER_SOURCE, "build_cortex_router");

    assert_eq!(
        routes_in(cortex_source),
        expected_routes(&[Owner::Cortex, Owner::Duplicate])
    );
}

#[test]
fn caddy_has_explicit_product_matchers_and_deny_fallbacks() {
    let apex = caddy_site("\nheyvera.org, www.heyvera.org {");
    let api = caddy_site("\napi.heyvera.org {");

    for site in [apex, api] {
        let site = site.replace("\r\n", "\n");
        assert!(site.contains("@heyvera_api path /api/health"));
        assert!(site.contains("handle /api/* {\n\t\trespond \"Not found\" 404"));
        assert!(site.contains("handle /v1/* {\n\t\trespond \"Not found\" 404"));
        assert!(!site.contains("handle /api/* {\n\t\treverse_proxy"));
        assert!(!site.contains("localhost:3402"));
    }

    assert!(api.contains("@cortex_api path /api/chat"));
    assert!(api.contains("/api/billing/referral/validate"));
    assert!(apex.contains("/api/clerk/webhooks"));
}

/// Negative probe: no `/v1/social/*` or `/v1/pulse/*` surface leaked back into
/// the Cortex router, and the routes the manifest promises are actually wired.
#[tokio::test]
async fn cortex_router_serves_only_the_manifest_and_no_socials_surface() {
    std::env::set_var("CORTEX_AUTH_DISABLED", "1");
    let temp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(temp.path().join(".cortex")).unwrap();
    std::env::set_var(
        "CORTEX_STATIC_DIR",
        temp.path().join("missing-cortex-static"),
    );
    let state = AppState::new(
        temp.path().join(".cortex/ledger.jsonl"),
        temp.path().to_path_buf(),
        None,
    )
    .await;
    let cortex = cortex_api::build_cortex_router(state);

    for socials_path in [
        "/v1/social/trending",
        "/v1/social/feed/home",
        "/v1/pulse/drafts",
        "/v1/pulse/chat",
    ] {
        let response = send(cortex.clone(), "GET", socials_path).await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "Socials route {socials_path} leaked into the Cortex router"
        );
    }

    let cortex_auth = send(cortex.clone(), "GET", "/api/auth/status").await;
    assert_ne!(cortex_auth.status(), StatusCode::NOT_FOUND);

    let cortex_health = send(cortex.clone(), "GET", "/v1/health").await;
    assert_eq!(response_json(cortex_health).await["service"], "cortex");

    let cortex_usage = send(cortex, "GET", "/api/billing/usage").await;
    assert_eq!(cortex_usage.status(), StatusCode::OK);
}
