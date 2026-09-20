# Security Policy

## Reporting a vulnerability

**Report privately. Do not open a public issue.**

Use GitHub's private vulnerability reporting:
[**Report a vulnerability**](https://github.com/1xmint/cortex/security/advisories/new)
— Security → Advisories → Report a vulnerability, on this repository.

That channel is private between you and the maintainers until an advisory is
published. It also gives us a place to draft a fix and a CVE request without
either being visible first.

If GitHub advisories are unavailable to you, email the repository owner and say
only that you have a security report and would like a private channel. Do not
put details in that first message.

### What to include

- What the issue is, and which component (`crates/api`, `crates/worker`,
  `crates/egress`, `crates/soma*`, the `cortex/` frontend, the
  deploy scripts, or CI).
- How to reproduce it, as concretely as you can. A failing test or a curl
  invocation is worth more than a description.
- What an attacker gets, and what they need in order to get it — in particular
  whether it needs an authenticated account, a worker credential, or neither.
- Any commit, branch, or deployed version you observed it on.

### What to expect

We will acknowledge your report, tell you whether we can reproduce it, and keep
you updated as we work on a fix. If we disagree that something is a
vulnerability we will say so and explain why, rather than going quiet.

We are a very small team. We do not promise a fixed response time, and we would
rather state that plainly than publish a number we cannot honour.

### No bug bounty

**There is no bug bounty programme and no monetary reward.** We do not pay for
reports. This is stated up front so nobody spends effort here expecting a
payout. Credit in the published advisory is offered, and can be declined.

## Scope

In scope: this repository's source, its GitHub Actions workflows, its container
images, and any Cortex instance the maintainers operate.

Out of scope:

- Denial of service through sheer volume, and anything requiring physical
  access or a compromised developer machine.
- Findings from automated scanners with no demonstrated impact.
- Vulnerabilities in third-party dependencies with no exploitable path through
  this code — report those upstream. Dependabot already watches this
  repository's dependency graph, and the `cargo-deny` and `npm-audit` CI jobs
  are required checks on `main`.
- Social engineering of maintainers or users.

Please do not test against production systems in a way that degrades service or
touches data belonging to anyone but you.

## Areas worth your attention

Not a claim that these are broken — a statement of where the trust boundaries
actually are, so a reviewer does not have to find them first:

- **Worker authentication** (`crates/api/src/ws.rs`, `crates/api/src/worker_key.rs`).
  A `cwk_` worker credential grants the right to execute steps. The anonymous
  worker path is closed behind `CORTEX_ALLOW_ANONYMOUS_WORKER`, which defaults
  to off; a deployment that sets it accepts unauthenticated workers on purpose.
- **The execution sandbox and egress mediator** (`crates/worker/src/sandbox`,
  `crates/egress`). Customer code runs here, and the mediator is what decides
  which hosts that code may reach.
- **The credit ledger and verification path** (`crates/api/src/verification_driver.rs`,
  the ledger in `crates/api/src/db.rs`). This is the money path.
- **Task completion gating.** Marking a task done is refused server-side without
  an evidence-backed verified run, in `require_evidence_for_done_transitions`
  (`crates/api/src/integrations.rs`). The client-side check is a UX pre-check
  and is not the boundary.
- **The Soma fence.** `soma` is behind a cargo feature, off by default. A build
  that enables it changes the trust model; see ADR-0003.
