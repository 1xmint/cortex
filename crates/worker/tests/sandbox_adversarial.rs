//! Adversarial tests for the execution boundary.
//!
//! The unit tests in `crates/worker/src/sandbox/` assert the *configuration* —
//! that the runner asks for an empty environment, one mount, no network,
//! dropped capabilities. These assert the *effect*: that a process inside the
//! sandbox actually cannot read a host secret, reach the Docker socket, open a
//! connection, write outside its workspace, or leave anything behind for the
//! next task.
//!
//! Both matter, and the second kind is the one that catches a runtime whose
//! semantics differ from what the API promised.
//!
//! # Running them
//!
//! These need a live container runtime and the sandbox image, so they are
//! gated: they run when `CORTEX_SANDBOX_IT=1` and are skipped otherwise, which
//! keeps the Windows development loop usable. **They run in CI** — a gated
//! adversarial test that never executes is worse than no test, because it
//! reads like coverage.
//!
//! ```bash
//! docker build -f Dockerfile.sandbox -t cortex/sandbox:test .
//! docker build -f Dockerfile.egress  -t cortex/egress:test .
//! CORTEX_SANDBOX_IT=1 CORTEX_SANDBOX_IMAGE=cortex/sandbox:test \
//!   CORTEX_EGRESS_IMAGE=cortex/egress:test \
//!   cargo test -p cortex-worker --test sandbox_adversarial
//! ```
//!
//! The egress tests reach a real registry. That is deliberate: a mediator
//! checked only against a local stub proves the stub is reachable, not that a
//! task can resolve a dependency and nothing else.

use cortex_core::execution_job::{
    BackendKind, BlockedReason, Budgets, CapabilityGrant, EffortApplication, ExecutionJob,
    IsolationClass, ModelRef, NetworkPolicy, ResourceProfile, EXECUTION_JOB_VERSION,
};
use cortex_core::provider::ProviderId;
use cortex_worker::sandbox::{
    ContainerSandbox, OutputStream, SandboxExit, SandboxRequest, SandboxRunner,
};

/// Skip unless a runtime is available and the test was asked for.
///
/// Returns the image to run. A missing image is a hard failure rather than a
/// skip when `CORTEX_SANDBOX_IT=1`: being asked to run these and silently not
/// running them is the failure mode this whole file exists to avoid.
fn enabled() -> Option<String> {
    if std::env::var("CORTEX_SANDBOX_IT").as_deref() != Ok("1") {
        return None;
    }
    Some(
        std::env::var("CORTEX_SANDBOX_IMAGE")
            .expect("CORTEX_SANDBOX_IT=1 requires CORTEX_SANDBOX_IMAGE"),
    )
}

fn job(image: &str, attempt: &str) -> ExecutionJob {
    ExecutionJob {
        job_id: uuid::Uuid::new_v4().to_string(),
        job_version: EXECUTION_JOB_VERSION,
        run_id: "run-it".to_string(),
        step_id: "step-it".to_string(),
        attempt_id: attempt.to_string(),
        lease_gen: 1,
        model_ref: ModelRef::uncatalogued("test-model"),
        backend_kind: BackendKind::Cli,
        effort: None,
        effort_applied: EffortApplication::NotRequested,
        budgets: Budgets::unquoted(),
        network_policy: NetworkPolicy::Deny,
        capability_grants: Vec::new(),
        context_bundle: None,
        quote_id: None,
        plan_receipt_id: None,
        image_ref: image.to_string(),
        isolation_class: IsolationClass::Container,
        resource_profile: ResourceProfile::default(),
        effective_egress: Some(Vec::new()),
        egress_mediator: None,
    }
}

/// Run a shell command in a sandbox over `workspace`, returning
/// (stdout+stderr, exit).
async fn run(
    image: &str,
    workspace: &std::path::Path,
    attempt: &str,
    script: &str,
    mutate: impl FnOnce(&mut ExecutionJob),
) -> (String, SandboxExit) {
    let mut job = job(image, attempt);
    mutate(&mut job);

    let runner = ContainerSandbox::new(image).expect("container runtime must be reachable");
    let request = SandboxRequest::new(workspace, "sh", vec!["-c".to_string(), script.to_string()]);

    let mut session = runner
        .submit(&job, &request)
        .await
        .expect("sandbox must start");

    let mut output = String::new();
    while let Some(line) = session.next_line().await {
        let tag = match line.stream {
            OutputStream::Stdout => "",
            OutputStream::Stderr => "[stderr] ",
        };
        output.push_str(tag);
        output.push_str(&line.text);
        output.push('\n');
    }
    let exit = session
        .wait()
        .await
        .expect("sandbox must report an outcome");
    (output, exit)
}

fn workspace() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cortex-it-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("workspace");
    dir
}

fn exited_nonzero(exit: &SandboxExit) -> bool {
    matches!(exit, SandboxExit::Exited { code } if *code != 0)
}

#[tokio::test]
async fn cannot_read_a_host_secret() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // A canary the host process can see and the sandbox must not. The
    // environment is empty rather than filtered, so nothing carries it in.
    std::env::set_var("CORTEX_IT_CANARY", "super-secret-value");

    let (output, _) = run(&image, &dir, "a1", "env; echo done", |_| {}).await;

    assert!(
        !output.contains("super-secret-value"),
        "a host environment variable reached the sandbox:\n{output}"
    );
    assert!(
        !output.contains("CORTEX_IT_CANARY"),
        "a host variable name reached the sandbox:\n{output}"
    );

    std::env::remove_var("CORTEX_IT_CANARY");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn cannot_reach_the_docker_socket() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // Reaching the socket is escape to full host control, so this is the
    // single highest-value assertion in the file.
    let (output, exit) = run(
        &image,
        &dir,
        "a2",
        "test -S /var/run/docker.sock && echo REACHABLE || echo absent",
        |_| {},
    )
    .await;

    assert!(
        !output.contains("REACHABLE"),
        "the Docker socket was mounted into the sandbox:\n{output}"
    );
    assert!(matches!(exit, SandboxExit::Exited { .. }));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn egress_is_denied_by_default() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // No allowlist, no grant. A connection that succeeds here means a task can
    // fetch a passing result or exfiltrate a private tree.
    let (output, exit) = run(
        &image,
        &dir,
        "a3",
        "getent hosts example.com && echo RESOLVED || echo blocked",
        |_| {},
    )
    .await;

    assert!(
        !output.contains("RESOLVED"),
        "the sandbox resolved a host with egress denied:\n{output}"
    );
    assert!(matches!(exit, SandboxExit::Exited { .. }));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_ungranted_allowlist_entry_opens_nothing() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // An allowlist without a matching capability grant must not open egress —
    // the two have to agree, or the allowlist is a self-service network.
    let (output, _) = run(
        &image,
        &dir,
        "a4",
        "getent hosts example.com && echo RESOLVED || echo blocked",
        |job| {
            job.network_policy = NetworkPolicy::Allowlist {
                hosts: vec!["example.com".to_string()],
            };
            // Deliberately no CapabilityGrant.
        },
    )
    .await;

    assert!(
        !output.contains("RESOLVED"),
        "an ungranted allowlist entry opened the network:\n{output}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn cannot_write_outside_the_workspace() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    let (output, _) = run(
        &image,
        &dir,
        "a5",
        "echo x > /etc/cortex-escape 2>/dev/null && echo WROTE || echo denied; \
         echo y > /work/allowed && echo wrote-workspace",
        |_| {},
    )
    .await;

    assert!(
        !output.contains("WROTE"),
        "the sandbox wrote outside its workspace:\n{output}"
    );
    assert!(
        output.contains("wrote-workspace"),
        "the workspace must be writable, or no task can do work:\n{output}"
    );
    assert!(
        dir.join("allowed").exists(),
        "the workspace write must land on the mounted worktree"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn no_state_persists_into_the_next_task() {
    let Some(image) = enabled() else { return };
    let first = workspace();
    let second = workspace();

    // Write a marker outside the mount, where a reused sandbox would keep it.
    let (_, _) = run(&image, &first, "a6", "echo marker > /tmp/leftover", |_| {}).await;

    let (output, _) = run(
        &image,
        &second,
        "a7",
        "cat /tmp/leftover 2>/dev/null && echo LEAKED || echo clean",
        |_| {},
    )
    .await;

    assert!(
        !output.contains("LEAKED"),
        "state from a previous task survived into the next sandbox:\n{output}"
    );

    let _ = std::fs::remove_dir_all(&first);
    let _ = std::fs::remove_dir_all(&second);
}

#[tokio::test]
async fn runs_unprivileged() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    let (output, _) = run(&image, &dir, "a8", "id -u", |_| {}).await;

    assert!(
        !output.trim().lines().any(|line| line.trim() == "0"),
        "the sandbox ran as root:\n{output}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn cannot_escalate_privileges() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // no-new-privileges plus cap_drop ALL. If a setuid path still works, the
    // unprivileged user is decorative.
    let (output, exit) = run(
        &image,
        &dir,
        "a9",
        "su root -c id 2>&1 || echo denied",
        |_| {},
    )
    .await;

    assert!(
        output.contains("denied") || exited_nonzero(&exit) || !output.contains("uid=0"),
        "privilege escalation succeeded inside the sandbox:\n{output}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_wall_clock_budget_terminates_a_hung_job() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // Invariant 14's cap, enforced at the boundary rather than trusted to the
    // agent. Without this the cap is decorative.
    let (_, exit) = run(&image, &dir, "a10", "sleep 600", |job| {
        job.budgets.wall_clock = std::time::Duration::from_secs(3);
    })
    .await;

    assert_eq!(
        exit,
        SandboxExit::BudgetExhausted,
        "a job that never exits must be terminated by its budget"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

// --- Scoped egress -------------------------------------------------------
//
// The mediator design in one sentence: the sandbox gets a network with no route
// off the host, and the mediator is the only thing on it that has one.
//
// Everything below exists to check that sentence rather than the configuration
// that is supposed to produce it. The one that matters most is
// `egress_a_direct_connection_bypassing_the_proxy_fails`: if a task can reach an
// allowlisted host without going through the mediator, then the allowlist is a
// suggestion and this whole feature is decorative.

/// The mediator image, built alongside the sandbox image in CI.
fn egress_image() -> String {
    std::env::var("CORTEX_EGRESS_IMAGE").unwrap_or_else(|_| "cortex/egress:test".to_string())
}

/// A job granted exactly the npm registry, which is the smallest real grant:
/// one host, HTTPS only.
fn npm_granted(job: &mut ExecutionJob) {
    job.network_policy = NetworkPolicy::Allowlist {
        hosts: vec!["registry.npmjs.org".to_string()],
    };
    job.capability_grants = vec![CapabilityGrant::ResolveDependencies {
        registries: vec!["npm".to_string()],
    }];
}

/// Run a script in a sandbox whose runner uses the locally built mediator.
async fn run_with_egress(
    image: &str,
    workspace: &std::path::Path,
    attempt: &str,
    script: &str,
    mutate: impl FnOnce(&mut ExecutionJob),
) -> (String, SandboxExit) {
    let mut job = job(image, attempt);
    mutate(&mut job);

    let runner = ContainerSandbox::new(image)
        .expect("container runtime must be reachable")
        .with_egress_image(egress_image());
    let request = SandboxRequest::new(workspace, "sh", vec!["-c".to_string(), script.to_string()]);

    let mut session = runner
        .submit(&job, &request)
        .await
        .expect("sandbox must start");

    let mut output = String::new();
    while let Some(line) = session.next_line().await {
        output.push_str(&line.text);
        output.push('\n');
    }
    let exit = session
        .wait()
        .await
        .expect("sandbox must report an outcome");
    (output, exit)
}

#[tokio::test]
async fn egress_an_allowed_host_is_reachable() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // The whole point of the PR. If this fails, no task can resolve a
    // dependency and the sandbox is safe and useless.
    let (output, _) = run_with_egress(
        &image,
        &dir,
        "e1",
        "curl -sS -o /dev/null -w 'code=%{http_code}\\n' --max-time 30 \
         https://registry.npmjs.org/ || echo UNREACHABLE",
        npm_granted,
    )
    .await;

    assert!(
        output.contains("code=200"),
        "the granted registry was not reachable through the mediator:\n{output}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn egress_a_denied_host_is_denied() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // Same network, same mediator, a host nobody granted.
    let (output, _) = run_with_egress(
        &image,
        &dir,
        "e2",
        "curl -sS -o /dev/null -w 'code=%{http_code}\\n' --max-time 30 \
         https://example.com/ || echo REFUSED",
        npm_granted,
    )
    .await;

    assert!(
        !output.contains("code=200"),
        "a host outside the allowlist was reachable:\n{output}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn egress_a_direct_connection_bypassing_the_proxy_fails() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // **The test that matters.** `--noproxy '*'` is exactly what a task would
    // do to escape: ignore the proxy variables entirely and dial the
    // allowlisted host itself. It must fail, and it must fail for lack of a
    // route rather than because anything inspected the request.
    //
    // If this ever passes, the allowlist is advice and every other test in this
    // section is measuring the honour system.
    let (output, _) = run_with_egress(
        &image,
        &dir,
        "e3",
        "curl -sS -o /dev/null -w 'code=%{http_code}\\n' --max-time 20 --noproxy '*' \
         https://registry.npmjs.org/ || echo NO_ROUTE",
        npm_granted,
    )
    .await;

    assert!(
        !output.contains("code=200"),
        "a task reached an allowlisted host without the mediator, so the \
         allowlist is not enforced:\n{output}"
    );
    assert!(
        output.contains("NO_ROUTE"),
        "the direct connection should have failed outright:\n{output}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn egress_unsetting_the_proxy_variables_does_not_restore_the_internet() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // The same claim from the task's point of view: the variables are a
    // convenience for well-behaved clients, never the boundary.
    let (output, _) = run_with_egress(
        &image,
        &dir,
        "e4",
        "unset HTTP_PROXY HTTPS_PROXY http_proxy https_proxy; \
         curl -sS -o /dev/null -w 'code=%{http_code}\\n' --max-time 20 \
         https://registry.npmjs.org/ || echo NO_ROUTE",
        npm_granted,
    )
    .await;

    assert!(
        !output.contains("code=200"),
        "unsetting the proxy variables restored real network access:\n{output}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn egress_raw_tcp_to_an_arbitrary_address_fails() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // Not HTTP at all. A task that opens a socket to a public resolver is
    // exfiltrating or command-and-controlling, and there must be no route for
    // it whatever protocol it speaks.
    let (output, _) = run_with_egress(
        &image,
        &dir,
        "e5",
        "nc -w 5 -z 1.1.1.1 443 && echo CONNECTED || echo NO_ROUTE",
        npm_granted,
    )
    .await;

    assert!(
        !output.contains("CONNECTED"),
        "raw TCP left the sandbox:\n{output}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn egress_an_allowed_host_on_an_ungranted_port_is_denied() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // The allowlist grants `registry.npmjs.org:443`. A host entry that
    // permitted every port would also permit an SSH daemon, a database, or an
    // internal admin port on the same name.
    //
    // `--proxytunnel` forces CONNECT for a plain http:// URL, which is how a
    // task would ask the mediator for port 80.
    let (output, _) = run_with_egress(
        &image,
        &dir,
        "e6",
        "curl -sS -o /dev/null -w 'code=%{http_code}\\n' --max-time 20 --proxytunnel \
         http://registry.npmjs.org/ || echo REFUSED",
        npm_granted,
    )
    .await;

    assert!(
        !output.contains("code=200"),
        "an ungranted port on an allowlisted host was reachable:\n{output}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn egress_a_non_connect_request_is_refused_not_forwarded() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // The mediator tunnels CONNECT and nothing else. Absolute-URI forwarding
    // would mean header, keep-alive, and chunked-body parsing in the one
    // component between a task and the internet. A client that asks for it is
    // told 405 rather than left to hang.
    let (output, _) = run_with_egress(
        &image,
        &dir,
        "e7",
        "curl -sS -o /dev/null -w 'code=%{http_code}\\n' --max-time 20 \
         http://registry.npmjs.org/ || echo REFUSED",
        npm_granted,
    )
    .await;

    assert!(
        output.contains("code=405") || output.contains("REFUSED"),
        "a non-CONNECT request was neither refused nor reported:\n{output}"
    );
    assert!(
        !output.contains("code=200"),
        "the mediator forwarded a plain HTTP request:\n{output}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn egress_an_unknown_registry_grant_is_refused() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // A grant naming something the registry table does not know justifies
    // nothing. Running the job anyway would give it less reach than its author
    // believed, which fails later and looks like a broken network rather than
    // a policy we declined to honour.
    let mut job = job(&image, "e8");
    job.network_policy = NetworkPolicy::Allowlist {
        hosts: vec!["evil.example.com".to_string()],
    };
    job.capability_grants = vec![CapabilityGrant::ResolveDependencies {
        registries: vec!["evil.example.com".to_string()],
    }];

    let runner = ContainerSandbox::new(&image)
        .expect("container runtime must be reachable")
        .with_egress_image(egress_image());
    let request = SandboxRequest::new(&dir, "sh", vec!["-c".to_string(), "true".to_string()]);

    let blocked = runner
        .submit(&job, &request)
        .await
        .err()
        .expect("an unknown registry must be refused, not silently narrowed");
    assert_eq!(
        blocked.reason,
        cortex_core::execution_job::BlockedReason::NetworkPolicyUnenforceable
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn egress_a_broken_mediator_blocks_rather_than_opening_a_network() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // Invariant 8's forbidden downgrade, in the exact place it would be
    // tempting: the proxy did not come up, so start the sandbox on a normal
    // network and let the task get on with it. That would silently undo the
    // sandbox, so it must refuse instead.
    let mut job = job(&image, "e9");
    npm_granted(&mut job);

    let runner = ContainerSandbox::new(&image)
        .expect("container runtime must be reachable")
        .with_egress_image("cortex/egress-does-not-exist:never-built");
    let request = SandboxRequest::new(&dir, "sh", vec!["-c".to_string(), "true".to_string()]);

    let blocked = runner
        .submit(&job, &request)
        .await
        .err()
        .expect("a mediator that cannot start must block the job");
    assert_eq!(
        blocked.reason,
        cortex_core::execution_job::BlockedReason::NetworkPolicyUnenforceable
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn egress_the_network_and_mediator_are_removed_after_the_attempt() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // A surviving network or mediator is the leftover state that makes reuse a
    // leak: one task's allowlist would be sitting there serving the next one.
    let attempt = "e10";
    let (_, _) = run_with_egress(&image, &dir, attempt, "true", npm_granted).await;

    let docker = bollard::Docker::connect_with_local_defaults().expect("runtime");
    let networks = docker
        .list_networks::<String>(None)
        .await
        .expect("can list networks");
    let leaked: Vec<String> = networks
        .into_iter()
        .filter_map(|n| n.name)
        .filter(|name| name.contains(&format!("cortex-egress-{attempt}-")))
        .collect();
    assert!(
        leaked.is_empty(),
        "the attempt's egress network survived it: {leaked:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn egress_records_the_effective_host_set_on_the_job() {
    // Not a runtime test: this is what a receipt will show, and it has to be
    // the resolved set rather than the requested one. Runs everywhere so the
    // claim is checked even without a container runtime.
    let mut job = job("cortex/runner@sha256:abc", "e11");
    job.network_policy = NetworkPolicy::Allowlist {
        hosts: vec![
            "crates.io".to_string(),
            "index.crates.io".to_string(),
            "evil.example.com".to_string(),
        ],
    };
    job.capability_grants = vec![CapabilityGrant::ResolveDependencies {
        registries: vec!["crates".to_string()],
    }];

    let endpoints: Vec<String> = cortex_worker::sandbox::policy::effective_endpoints(&job)
        .iter()
        .map(|e| e.to_string())
        .collect();

    assert!(endpoints.contains(&"crates.io:443".to_string()));
    assert!(endpoints.contains(&"index.crates.io:443".to_string()));
    assert!(
        !endpoints.iter().any(|e| e.starts_with("evil.example.com")),
        "an entry no grant justifies must not appear on the receipt: {endpoints:?}"
    );
    assert!(
        endpoints.iter().all(|e| e.ends_with(":443")),
        "every entry carries the port it was granted on: {endpoints:?}"
    );
}

// --- F7: the sandboxed CLI's route to the model ---
//
// The gap these close, stated because it is the reason the wave exists: the
// tests above assert that a *registry* grant is enforced. Nothing asserted that
// a step routed to a provider could reach that provider's API, so a sandbox
// that had never been able to run an agent passed every one of them. These
// assert against a real runtime, on the same network topology, using the same
// grant machinery.

/// A job granted exactly one provider, as the scheduler now issues.
fn provider_granted(job: &mut ExecutionJob, provider: ProviderId) {
    let plan = cortex_core::egress::derive_provider_egress(provider);
    job.network_policy = plan.network_policy;
    job.capability_grants = plan.capability_grants;
}

#[tokio::test]
async fn egress_a_claude_grant_cannot_reach_the_supplier_directly() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // Claude traffic must now traverse the private Cortex gateway. Even if a
    // repository-controlled process learns a supplier credential by some
    // other route, the sandbox grant must not let it bypass reservations by
    // contacting Anthropic directly.
    let (output, _) = run_with_egress(
        &image,
        &dir,
        "p1",
        "curl -sS -o /dev/null -w 'code=%{http_code}\n' --max-time 30 \
         https://api.anthropic.com/v1/messages || echo UNREACHABLE",
        |job| provider_granted(job, ProviderId::Claude),
    )
    .await;

    assert!(
        output.contains("UNREACHABLE"),
        "a Claude-routed sandbox reached Anthropic directly:\n{output}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn egress_a_step_routed_to_one_provider_cannot_reach_another() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // The narrowness half of the decision. A provider grant is one host, not a
    // provider list — so a step routed to Claude must not be able to spend a
    // key against OpenAI, and must not be able to reach Gemini either.
    let (output, _) = run_with_egress(
        &image,
        &dir,
        "p2",
        "curl -sS -o /dev/null -w 'openai=%{http_code}\n' --max-time 20 \
           https://api.openai.com/v1/models || echo OPENAI_REFUSED; \
         curl -sS -o /dev/null -w 'gemini=%{http_code}\n' --max-time 20 \
           https://generativelanguage.googleapis.com/ || echo GEMINI_REFUSED",
        |job| provider_granted(job, ProviderId::Claude),
    )
    .await;

    assert!(
        output.contains("OPENAI_REFUSED"),
        "a step routed to Claude reached OpenAI:\n{output}"
    );
    assert!(
        output.contains("GEMINI_REFUSED"),
        "a step routed to Claude reached Gemini:\n{output}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn egress_a_provider_grant_does_not_open_a_registry() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // The two grants are separate, and this is that separation observed at the
    // boundary rather than asserted in a unit test: a provider grant opens the
    // provider and nothing a repository would have justified.
    let (output, _) = run_with_egress(
        &image,
        &dir,
        "p3",
        "curl -sS -o /dev/null -w 'npm=%{http_code}\n' --max-time 20 \
         https://registry.npmjs.org/ || echo NPM_REFUSED",
        |job| provider_granted(job, ProviderId::Claude),
    )
    .await;

    assert!(
        output.contains("NPM_REFUSED"),
        "a provider grant opened a package registry:\n{output}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn egress_an_unknown_provider_grant_is_refused() {
    let Some(image) = enabled() else { return };
    let dir = workspace();

    // Same rule as the unknown-registry case, and refused for a sharper
    // reason: a provider name this build cannot expand means no route and no
    // credential, so running would produce a mystery failure charged to the
    // customer instead of a typed refusal against Cortex.
    let mut job = job(&image, "p4");
    job.network_policy = NetworkPolicy::Allowlist {
        hosts: vec!["api.anthropic.com".to_string()],
    };
    job.capability_grants = vec![CapabilityGrant::ReachProvider {
        provider: "not-a-provider".to_string(),
    }];

    let runner = ContainerSandbox::new(&image)
        .expect("container runtime must be reachable")
        .with_egress_image(egress_image());
    let request = SandboxRequest::new(&dir, "sh", vec!["-c".to_string(), "true".to_string()]);

    let refusal = runner
        .submit(&job, &request)
        .await
        .err()
        .expect("an unknown provider grant must be refused, not narrowed");
    assert_eq!(refusal.reason, BlockedReason::NetworkPolicyUnenforceable);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn egress_records_the_provider_grant_separately_from_the_registry_grant() {
    // Not a runtime test, and deliberately so: this is what a receipt shows,
    // and it must be checkable without a container runtime. A reader has to be
    // able to tell which grant opened which host after the fact.
    let mut job = job("cortex/runner@sha256:abc", "p5");
    let ecosystem = cortex_core::egress::derive_egress(
        &cortex_core::egress::EcosystemManifests {
            cargo: true,
            ..Default::default()
        },
        true,
    );
    let provider = cortex_core::egress::derive_provider_egress(ProviderId::Claude);
    let union = cortex_core::egress::EgressPlan::union(&ecosystem, &provider);
    job.network_policy = union.network_policy;
    job.capability_grants = union.capability_grants;

    let endpoints: Vec<String> = cortex_worker::sandbox::policy::effective_endpoints(&job)
        .iter()
        .map(|e| e.to_string())
        .collect();

    assert!(endpoints.contains(&"cortex.heyvera.org:443".to_string()));
    assert!(endpoints.contains(&"index.crates.io:443".to_string()));

    // And the grants are still two things, not one merged host list.
    let providers = job
        .capability_grants
        .iter()
        .filter(|g| matches!(g, CapabilityGrant::ReachProvider { .. }))
        .count();
    let registries = job
        .capability_grants
        .iter()
        .filter(|g| matches!(g, CapabilityGrant::ResolveDependencies { .. }))
        .count();
    assert_eq!(providers, 1, "the provider grant was merged away");
    assert_eq!(registries, 1, "the registry grant was merged away");
}
