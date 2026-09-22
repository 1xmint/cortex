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
        108,
        "cortex-standalone keeps 93 cortex + 15 duplicate routes"
    );
    assert_eq!(
        manifest
            .iter()
            .map(|contract| contract.path.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        108,
        "every path template must have exactly one owner"
    );
    assert_eq!(
        manifest
            .iter()
            .filter(|contract| contract.owner == Owner::Cortex)
            .count(),
        93
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

/// The Caddyfile is the edge, and since the split it carries one site. The
/// guarantee it has to keep is that every path it forwards is a path the
/// backend still serves. `/v1` is therefore named route by route: a broad
/// `/v1/*` proxy would forward the deleted `/v1/social/*` and `/v1/pulse/*`
/// surface to a backend that no longer answers it.
///
/// This replaced a test that asserted the HeyVera sites explicitly 404'd the
/// Cortex paths and vice versa. Those sites live in the HeyVera repository
/// now, so that boundary is not this file's to prove.
#[test]
fn caddy_forwards_only_paths_the_cortex_backend_serves() {
    for host in [
        "\nheyvera.org, www.heyvera.org {",
        "\napi.heyvera.org {",
        "\npulse.heyvera.org {",
    ] {
        assert!(
            !CADDYFILE.contains(host),
            "{host:?} belongs to the HeyVera repository and must not come back"
        );
    }
    assert_eq!(
        CADDYFILE.matches(".heyvera.org {").count(),
        1,
        "the Caddyfile serves the Cortex host and nothing else"
    );
    assert!(!CADDYFILE.contains("localhost:3402"), "legacy ClawNet port");

    // Comments here describe the sites that were removed, so judge the
    // directives alone.
    let site = caddy_site("\ncortex.heyvera.org {").replace("\r\n", "\n");
    let directives = site
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        directives.contains("@api path /api/* /v1/health /v1/ready"),
        "the two Cortex /v1 routes are named one by one"
    );
    for absent in ["/v1/*", "/v1/social", "/v1/pulse"] {
        assert!(
            !directives.contains(absent),
            "{absent} would reach a backend that no longer serves it"
        );
    }

    // Real routes that are deliberately off the public hostname: Prometheus
    // scrapes over the private network, and the provider endpoint is the
    // worker's. Publishing either is a change in exposure, not a fix.
    for private in ["/metrics", "/internal/"] {
        assert!(
            !directives.contains(private),
            "{private} must not be exposed on the public hostname"
        );
    }

    let upstreams = directives
        .lines()
        .filter_map(|line| line.trim().strip_prefix("reverse_proxy "))
        .map(|rest| rest.trim_end_matches('{').trim())
        .collect::<Vec<_>>();
    assert_eq!(
        upstreams,
        vec!["localhost:3001"],
        "cortex-api is the only backend this file knows"
    );
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

    let cortex_admin = send(cortex.clone(), "GET", "/api/admin/containers").await;
    assert_ne!(cortex_admin.status(), StatusCode::NOT_FOUND);

    let cortex_health = send(cortex.clone(), "GET", "/v1/health").await;
    assert_eq!(response_json(cortex_health).await["service"], "cortex");

    let cortex_usage = send(cortex, "GET", "/api/billing/usage").await;
    assert_eq!(cortex_usage.status(), StatusCode::OK);
}
