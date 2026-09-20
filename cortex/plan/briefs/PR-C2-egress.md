# PR C2 — Scoped egress: a per-task host allowlist that actually holds

> Self-contained. Do not read `HARNESS-EXCELLENCE-PLAN-2026-08.md` to execute
> this. Plan sections are cited only for provenance if a decision is challenged.
> Source: Phase 32.4 (isolation) and Phase 32.2 (what we require of customers).

## Objective

PR C made the sandbox safe by making it unusable. A job that asks for network
access is refused with `NetworkPolicyUnenforceable`
(`crates/worker/src/sandbox/container.rs`), because enforcing a host allowlist
needs a mediator that did not exist, and a Docker network alone grants the whole
internet under the name of an allowlist. Refusing was the honest answer.

The consequence is that **no task can resolve a dependency**. No `npm install`,
no `cargo fetch`, no `pip install`, no `go mod download`. A sandbox that cannot
fetch a dependency cannot build most repositories, so Cortex cannot do real
work. This PR is the gate on everything downstream.

After this PR a job carrying `NetworkPolicy::Allowlist { hosts }` **runs**, and
reaches exactly those hosts and nothing else — including when the task actively
tries to reach something else.

## Do not do

- **Do not implement this by attaching a Docker network and filtering in
  userspace.** A task that ignores the proxy configuration must still fail. If
  the only thing standing between the task and the internet is an environment
  variable the task can unset, this PR has not been done.
- **Do not terminate TLS.** No MITM certificate authority, no payload
  inspection. The mediator sees the host from `CONNECT` and nothing else. A
  mediator that can read the traffic is a mediator that can leak it.
- **Do not give the mediator any credential the task could reach.** It holds
  the allowlist and nothing else. Registry tokens, Git credentials, and provider
  keys stay where they are.
- **Do not widen `sanctioned_env()` into a general environment passthrough.**
  Proxy variables are a convenience for well-behaved clients; they are never the
  enforcement. See Design decision 2.
- **Do not add wildcard or suffix matching in this PR.** Exact host names only.
  A `*.example.com` rule is a different security argument and belongs in its own
  change with its own tests.
- **Do not touch lifecycle states, billing, or the verifier.** Those are PRs A,
  F, and B.
- Do not touch the HeyVera Socials lane.

## Prerequisites

**Merged:** PR C (#502). PR A (#505) is not required, but if it has landed, a
mediator failure is recorded as `execution_failed` rather than as the task
failing — check which is true when you start.

**Verify present before starting:**

1. `crates/worker/src/sandbox/container.rs` — `submit` still returns
   `Blocked::new(BlockedReason::NetworkPolicyUnenforceable, …)` when
   `effective_hosts(job)` is non-empty. That refusal is what this PR replaces.
2. `crates/worker/src/sandbox/container.rs` — `build_config` still hard-codes
   `network_mode = "none"` and `network_disabled = Some(true)`, with a comment
   saying the deny is unconditional so a bug in the `submit` check cannot open
   the sandbox. **That property must survive**: the deny stays the default and
   the network is attached only on the path that was explicitly granted.
3. `crates/worker/src/sandbox/policy.rs` — `effective_hosts` intersects the
   allowlist with the registries named in a `CapabilityGrant::ResolveDependencies`,
   and `ungranted_hosts` reports the difference. Both are already correct and
   are the input to this PR, not something to redesign.
4. `crates/core/src/execution_job.rs` — `NetworkPolicy::{Deny, Allowlist}` and
   `CapabilityGrant::ResolveDependencies { registries }` exist, and
   `network_policy` is already a column on `execution_jobs` (migration v62).
5. `.github/workflows/ci.yml` — the `sandbox` job exists, is a **required**
   check as of #504, builds `Dockerfile.sandbox`, and asserts at least ten
   adversarial tests actually ran. New tests go in that job and raise that
   floor.
6. Current max migration — check it. v63 after PR A. `schema_version` is a
   single counter shared with the HeyVera Socials product; claim the next free
   number at rebase, not at design time.

## Files expected to change

| Path | Change |
|---|---|
| `Dockerfile.egress` *(new)* | The mediator image. Minimal base, one static binary, no shell, runs as a non-root user. Pinned by digest in production, same as the runner image. |
| `crates/worker/src/sandbox/egress.rs` *(new)* | Lifecycle of the per-attempt mediator: create the internal network, start the mediator, wait for it to be ready, hand back the endpoint, tear both down. Teardown attempted on every path including timeout and kill, matching `ContainerSandbox::remove`. |
| `crates/worker/src/sandbox/policy.rs` | `needs_network` already answers whether a mediator is required. Add the port policy (Design decision 4) and the registry-alias expansion (Design decision 5). |
| `crates/worker/src/sandbox/container.rs` | Replace the `NetworkPolicyUnenforceable` refusal with: provision the mediator, attach the sandbox to the **internal** network only, keep `network_mode` deny on every other path. Refuse — still `NetworkPolicyUnenforceable` — if the mediator cannot be provisioned. |
| `crates/core/src/execution_job.rs` | Record what actually mediated: the mediator image ref and the effective host set. A receipt that says "allowlist" without saying *which hosts* and *what enforced it* is not a receipt. |
| `crates/api/src/db.rs` | Migration for the two new `execution_jobs` columns. Re-check the counter. |
| `crates/worker/tests/sandbox_adversarial.rs` | The matrix below. These are the deliverable as much as the code is. |
| `docs/adr/` | An ADR for the mediator: why topology and not configuration, why `CONNECT` and not interception, why the mediator is in the trust boundary. |

## Invariants this PR must not violate

Copied verbatim from the plan's invariant list. Not paraphrased.

> 8. **If a required isolation cannot be obtained, dispatch fails closed and
>    explains the blocker.** A sandbox that cannot be established is never
>    silently downgraded to a weaker one.

> 15. **Default-deny egress, per-task allowlist.** A task reaches the hosts its
>     job names and no others. Package-registry access means exactly the
>     registry.

> 19. **Anything that mediates a task's access is inside the trust boundary.**
>     It does not execute code supplied by the task and does not hold
>     credentials the task can reach.

> 4. **Every receipt names an immutable source tree, an image, a resource
>    profile, and the argv that produced it.** What was reachable is part of
>    what produced it.

## Design decisions already made

Conclusions only.

1. **Enforcement is network topology, not client configuration.**

   The sandbox joins a Docker network created with `internal: true`, which has
   no route off the host. The mediator is dual-homed: that internal network and
   a normal bridge. The sandbox's only reachable next hop is the mediator's
   address. A task that unsets `HTTPS_PROXY` and opens a raw socket to an
   allowlisted host's IP gets no route — not a policy decision, an absence of a
   path.

   This is the whole argument for the design. Anything that relies on the task
   cooperating is not enforcement.

2. **`HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY` are set in the sandbox
   environment, and they are a convenience.**

   `sanctioned_env()` returns an empty vector today, and the comment explaining
   why — a filtered environment is a denylist somebody must keep correct forever
   — remains right. These three variables are an exception with a stated reason:
   they carry no secret, they name an address the task can already discover, and
   without them every well-behaved package manager would fail for a reason that
   looks like a bug. Add them explicitly, in one place, with that reasoning in a
   comment. Do not open a general passthrough.

3. **The mediator speaks `CONNECT`, and only `CONNECT`.**

   The client names the host; the mediator resolves it. The task never
   influences which address the name resolves to, which closes DNS rebinding
   without any special case. TLS is tunnelled, never terminated: the mediator
   sees a host and a port and forwards bytes.

   Absolute-URI forwarding — the `GET http://host/path` form a client uses for
   plain `http://` URLs through a proxy — is **not** implemented. Handling it
   means parsing and re-emitting headers, keep-alive, and chunked bodies, which
   is a meaningful amount of protocol surface inside the one component that
   sits between a task and the internet. Every registry this PR grants is
   HTTPS. A `CONNECT host:80` tunnel still works if an entry names port 80, so
   nothing is lost except the parsing.

   `HTTP_PROXY` is still set, and the mediator answers a non-`CONNECT` request
   with `405 Method Not Allowed` and a one-line body saying why. An explicit
   refusal is better than the hang a client would get from an unset variable.

4. **Ports are part of the allowlist, and the default is 443 only.**

   A host entry permits `:443`. Permitting `:80` requires the entry to say so.
   An allowlist that names a host but not a port is an allowlist for every
   service on that host, including an SSH daemon or a database.

5. **A registry grant expands to a fixed host set defined in this repository,
   not to whatever the task asks for.**

   `CapabilityGrant::ResolveDependencies { registries: ["npm"] }` expands to the
   exact hosts npm needs, from a table in `policy.rs`. A grant naming a raw
   hostname the table does not know is refused rather than trusted. This is what
   "package-registry access means exactly the registry" means in code: the task
   picks a registry by name, and we decide what that name reaches.

   Start with npm, crates.io, PyPI, and Go module proxy. Each entry carries a
   comment naming why every host in it is required.

6. **One mediator and one network per attempt, torn down with it.**

   Named from `attempt_id` and `lease_gen`, the same key `execution_jobs`
   already uses for one logical execution. A shared mediator would let one
   task's allowlist serve another's traffic, and a leaked network is the
   leftover state that makes reuse a leak. Teardown is attempted on every path;
   a survivor is a bug, not a race.

7. **The mediator cannot be provisioned ⇒ the job is blocked, not downgraded.**

   Same `BlockedReason::NetworkPolicyUnenforceable`, now meaning "we could not
   stand up the mediator" rather than "we never built one". Invariant 8. The
   tempting failure — start the sandbox with a normal bridge network because the
   proxy did not come up — is the exact thing PR C refused to do and must stay
   refused.

8. **The deny stays unconditional on every path that did not explicitly grant.**

   `build_config`'s hard-coded `network_mode: "none"` is load-bearing: it means
   a bug in the granting path cannot open the sandbox by accident. Keep the
   default and add the grant as a separate, explicit branch. Do not make the
   network mode a variable that defaults to open.

9. **The effective host set and the mediator image go on the job and the
   receipt.**

   `network_policy` on `execution_jobs` records what was *asked for*. Two new
   columns record what was *enforced*: the resolved host:port set after grant
   intersection and alias expansion, and the mediator image ref. A customer
   asking "what could this task reach" gets an answer from the receipt rather
   than from a reconstruction.

## Schema / migration

Two columns on `execution_jobs`, both nullable because rows written before this
PR genuinely do not have them and a plausible default would be a lie:

```sql
ALTER TABLE execution_jobs ADD COLUMN effective_egress TEXT;   -- JSON: ["host:port", …]
ALTER TABLE execution_jobs ADD COLUMN egress_mediator TEXT;    -- image ref, digest-pinned
```

`effective_egress` is `'[]'` — not NULL — for a job that ran under `Deny`, so
"nothing was reachable" is distinguishable from "we did not record it".

Claim the next free `schema_version` at rebase. v63 is taken by PR A.

## Test matrix

All in the `sandbox` CI job, which requires a real container runtime. The job
asserts a minimum count of adversarial tests; raise that floor to match.

| Test | Catches |
|---|---|
| `egress::allowed_host_is_reachable` | The whole PR being a no-op that just refuses more politely. |
| `egress::denied_host_is_denied` | The allowlist not being consulted. |
| `egress::direct_connection_to_allowed_host_ip_fails` | **The bypass test.** Raw socket to the allowlisted host's own IP, proxy variables unset. Must fail with no route — this is the one that proves enforcement is topology, not configuration. |
| `egress::unsetting_proxy_env_does_not_restore_the_internet` | The same claim from the task's point of view. |
| `egress::external_dns_does_not_resolve_inside_the_sandbox` | The sandbox resolving names for itself, which would let it pick the address. |
| `egress::allowed_host_on_a_non_allowlisted_port_is_denied` | A host entry silently permitting every service on that host. |
| `egress::forged_host_header_does_not_widen_the_allowlist` | Matching on something the task controls after `CONNECT`. |
| `egress::ungranted_allowlist_entry_opens_nothing` | The existing grant intersection regressing — an allowlist entry with no capability grant must still reach nothing. |
| `egress::deny_policy_gets_no_mediator_and_no_network` | A mediator being provisioned for jobs that never asked for one. |
| `egress::unknown_registry_alias_is_refused` | A grant naming an arbitrary hostname being trusted. |
| `egress::mediator_failure_blocks_rather_than_opening_a_network` | The downgrade in invariant 8 — the failure mode that would quietly undo PR C. |
| `egress::network_and_mediator_are_removed_after_the_attempt` | A leaked network or container surviving into the next task. |
| `egress::sandbox_cannot_reach_the_docker_socket_or_host_gateway` | The internal network still exposing the host itself. |
| `egress::receipt_records_the_effective_host_set` | A receipt that says "allowlist" without saying which hosts. |
| `egress::non_connect_request_is_refused_not_forwarded` | Absolute-URI forwarding creeping in, or a client hanging instead of being told no. |

Run locally with `CORTEX_SANDBOX_IT=1` and a container runtime. Rust:
`cargo +stable-x86_64-pc-windows-gnullvm test -p cortex-worker`. Never run
`cargo fmt` on this repo.

## Done criteria

- [ ] A job with `NetworkPolicy::Allowlist` and a matching grant **runs**, and
      reaches exactly the granted hosts.
- [ ] `grep -n "NetworkPolicyUnenforceable" crates/worker/src/sandbox/container.rs`
      shows it only on the mediator-provisioning failure path.
- [ ] `build_config` still denies the network by default; the grant is a
      separate explicit branch and not a defaulted variable.
- [ ] `sanctioned_env()` gained exactly three variables, each with a comment
      saying why it is not a secret and why it is not the enforcement.
- [ ] The mediator implements `CONNECT` and refuses every other method with
      `405`. No header parsing, no body rewriting.
- [ ] The mediator image runs no task-supplied code and holds no credential the
      task can reach; the ADR says so and the Dockerfile shows it.
- [ ] `effective_egress` and `egress_mediator` are recorded for every job,
      `'[]'` for `Deny`.
- [ ] Every test in the matrix exists, is named as written, and passes in the
      `sandbox` CI job — including the bypass test, which is the one that
      matters.
- [ ] The adversarial-test floor in `.github/workflows/ci.yml` was raised.
- [ ] Migration applies cleanly on a fresh database and on a copy of the current
      production schema; the fresh-database assertion in `db.rs` is updated.
- [ ] All seven required checks green: `heyvera`, `rust`, `cortex`,
      `npm-audit (cortex)`, `npm-audit (heyvera)`, `cargo-deny`, `sandbox`.
- [ ] `cargo fmt` was **not** run.

## Open questions

1. **Registry host sets drift.** npm, crates.io, and PyPI each move CDN hosts
   occasionally, and a stale table looks to a customer like a broken build. The
   table is the correct place for the decision, but it needs an owner and a
   review cadence. Recorded here rather than solved; it is an operations
   question, not a design one.
2. **Container-only.** The mediator design assumes a container runtime with
   user-defined networks. The microVM phase will need its own answer, most
   likely a vsock-attached mediator. The `SandboxRunner` trait carries no
   container vocabulary today and this PR must not add any, so the swap stays
   possible.
