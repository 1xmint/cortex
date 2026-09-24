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
        110,
        "cortex-standalone keeps 95 cortex + 15 duplicate routes"
    );
    assert_eq!(
        manifest
            .iter()
            .map(|contract| contract.path.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        110,
        "every path template must have exactly one owner"
    );
    assert_eq!(
        manifest
            .iter()
            .filter(|contract| contract.owner == Owner::Cortex)
            .count(),
        95
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

/// The Caddyfile is the edge, and it carries two sites. `api.heyvera.org` is
/// shared with the HeyVera repository; this repo does not own or list
/// HeyVera's routes on that host, but it does have to prove its own claim on
/// it is exactly `/api/*`. The provider gateway's own host claims exactly
/// `/internal/provider/*`, so sandbox code allowed to reach it reaches
/// nothing else. Both go to the same backend, and nothing either lists (in
/// particular `/metrics`) is exposed that shouldn't be.
///
/// `cortex.heyvera.org` used to be a site here too; it is a Cloudflare
/// Pages static site now and no longer reaches this host, so its block was
/// deleted rather than kept unserved.
///
/// This replaced a test that asserted the HeyVera sites explicitly 404'd the
/// Cortex paths and vice versa. Those sites live in the HeyVera repository
/// now, so that boundary is not this file's to prove.
#[test]
fn caddy_forwards_only_paths_the_cortex_backend_serves() {
    for host in [
        "\nheyvera.org, www.heyvera.org {",
        "\npulse.heyvera.org {",
        "\ncortex.heyvera.org {",
    ] {
        assert!(
            !CADDYFILE.contains(host),
            "{host:?} belongs to the HeyVera repository, or moved to Cloudflare Pages, and must not come back"
        );
    }
    assert_eq!(
        CADDYFILE.matches(".heyvera.org {").count(),
        2,
        "the Caddyfile serves exactly the shared api.heyvera.org host and the gateway's own host"
    );
    // Judge directives, not comments: the header comment names HeyVera's
    // ports to explain what lives on the shared host.
    let file_directives = CADDYFILE
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !file_directives.contains("localhost:3402"),
        "legacy ClawNet port; this repo does not own HeyVera's routes on the shared host"
    );

    // The provider gateway has a host of its own, so sandbox code that may
    // reach it reaches nothing else. The constant and the Caddyfile must
    // agree on where that is.
    assert_ne!(
        cortex_core::egress::PROVIDER_GATEWAY_HOST,
        "api.heyvera.org",
        "the gateway must not share a host with the product API"
    );
    let gateway_host = format!("\n{} {{", cortex_core::egress::PROVIDER_GATEWAY_HOST);
    assert!(
        CADDYFILE.contains(&gateway_host),
        "the provider gateway host constant and the Caddyfile must agree on where the gateway lives"
    );
    assert_cortex_site(&gateway_host, &["/internal/provider/*"]);

    // HeyVera's routes live on the shared host in production but are not in
    // this file, so this is the complete list this repo may claim there.
    assert_cortex_site("\napi.heyvera.org {", &["/api/*"]);
}

/// One Caddy site's `handle` paths are exactly `paths`, all to the one
/// Cortex backend, with a 404 for anything else, no `/metrics`, and the
/// security headers. Comments describe removed sites and HeyVera's routes,
/// so only directives are judged.
fn assert_cortex_site(label: &str, paths: &[&str]) {
    let site = caddy_site(label).replace("\r\n", "\n");
    let directives = site
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");

    let handle_paths: Vec<&str> = directives
        .lines()
        .filter_map(|line| line.trim().strip_prefix("handle "))
        .map(|rest| rest.trim_end_matches('{').trim())
        .filter(|path| !path.is_empty())
        .collect();
    assert_eq!(
        handle_paths, paths,
        "{label:?} may claim exactly these paths and no others"
    );

    assert!(
        !directives.contains("/metrics"),
        "/metrics must not be exposed on {label:?}; Prometheus scrapes over the private network"
    );

    assert!(
        directives.contains("respond \"Not found\" 404"),
        "an unmatched path on {label:?} (including HeyVera's, which this repo does not list) must not silently fall through to Cortex"
    );

    let upstreams = directives
        .lines()
        .filter_map(|line| line.trim().strip_prefix("reverse_proxy "))
        .map(|rest| rest.trim_end_matches('{').trim())
        .collect::<Vec<_>>();
    assert_eq!(
        upstreams,
        vec!["localhost:3001"; paths.len()],
        "every Cortex route on {label:?} goes to the one backend, and no other backend is named"
    );

    assert!(
        directives.contains("X-Content-Type-Options nosniff")
            && directives.contains("X-Frame-Options DENY")
            && directives.contains("Referrer-Policy strict-origin-when-cross-origin"),
        "{label:?} must keep its security headers"
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
