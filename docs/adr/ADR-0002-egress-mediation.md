# ADR-0002 — Scoped egress is enforced by topology, not by configuration

**Status:** accepted
**Date:** 2026-08-09
**Follows:** the `NetworkPolicyUnenforceable` refusal introduced with the
execution sandbox.

## Context

The sandbox denied all egress. A job carrying `NetworkPolicy::Allowlist` was
refused outright, with the reason `NetworkPolicyUnenforceable`, because a Docker
network alone grants the whole internet and calling that an allowlist would have
been a lie in the receipt. Refusing was correct.

It also meant no task could run `npm install`, `cargo fetch`, `pip install`, or
`go mod download`. A sandbox that cannot resolve a dependency cannot build most
repositories. The boundary was sound and the product could not do its job.

The requirement is invariant 15: a task reaches the hosts its job names and no
others, and package-registry access means exactly the registry.

## Decision

**The sandbox joins a network with no route off the host, and a mediator we
control is the only thing on that network that has one.**

Per attempt:

1. A Docker network created with `internal: true`. Docker installs no gateway
   for such a network, so nothing on it can reach anything off the host by any
   route at all.
2. The **egress mediator** container, attached to that network *and* to
   `bridge`. It is the only dual-homed member.
3. The sandbox container, attached to the internal network and nothing else.

A task that unsets `HTTPS_PROXY` and opens a raw socket to an allowlisted host's
IP address does not get filtered. It gets no route. That distinction is the
whole decision: **anything that depends on the task cooperating is not
enforcement.**

### Why not a userspace filter, iptables rules, or a DNS allowlist

- A **userspace filter the task is asked to use** is advice. The first thing a
  motivated task does is stop using it.
- **Per-task iptables rules** would work, but they live on the host, they are
  matched on addresses rather than names, and the rule set becomes shared
  mutable state that every concurrent attempt writes into. A leaked rule is
  silent and outlives the attempt.
- A **DNS allowlist** filters names, and the task chooses the address. It is
  bypassed by hardcoding an IP.

Topology has none of those failure modes and needs no host state: the network
is created and destroyed with the attempt.

### The mediator speaks `CONNECT`, and only `CONNECT`

It reads a `CONNECT host:port` line, matches it against an allowlist supplied on
argv, opens a socket, and copies bytes.

- **No TLS termination.** No certificate authority the task must trust, no
  plaintext in the mediator's memory. A mediator that can read what it forwards
  is a mediator that can leak it. The task's own client validates the
  certificate end to end, against the host it named.
- **The mediator resolves the name.** The task never gets to say which address a
  permitted name points at, which closes DNS rebinding without a special case
  for it.
- **The `Host` header is ignored.** It arrives after `CONNECT` and is
  client-controlled; matching on it would let a task name one host in the
  tunnel request and reach another.
- **Absolute-URI forwarding is not implemented.** Handling `GET http://host/path`
  means parsing and re-emitting headers, keep-alive, and chunked bodies — a
  meaningful amount of protocol surface inside the one component that sits
  between model-authored code and the internet, for no gain: every registry we
  grant is HTTPS, and `CONNECT host:80` still works where an entry names port
  80. A non-`CONNECT` request is answered `405`.

### Ports are half of a grant

An allowlist entry names a host and a port. An entry with no port means `:443`
and nothing else. A host entry that permitted every port would also permit an
SSH daemon, a database, or an internal admin interface on the same name.

### A registry name expands to a host set we define

`ResolveDependencies { registries: ["crates"] }` expands to the three hosts
cargo actually needs, from a table in `policy.rs`. A grant naming a hostname the
table has never heard of expands to nothing and **blocks the job** rather than
running it with less reach than its author believed — a job that quietly reaches
less than expected fails later and looks like a broken network instead of a
policy we declined to honour.

This is what "package-registry access means exactly the registry" is in code:
the task picks a registry by name, and we decide what that name reaches.

### The mediator is inside the trust boundary

Invariant 19. It runs no code supplied by the task, and it holds no credential
the task could reach — its entire configuration is the allowlist on argv. The
image is `scratch` plus one statically linked binary: no shell, no package
manager, no interpreter, and no CA bundle, because it never validates a
certificate. Its only dependency is `tokio`, because every dependency it carries
is a dependency inside the trust boundary.

### Failure blocks; it never downgrades

If the network cannot be created, the mediator cannot be started, or it does not
report ready, the job is `Blocked` with `NetworkPolicyUnenforceable`. There is
deliberately no path that starts the sandbox on a normal network because the
proxy did not come up. That is invariant 8's forbidden downgrade, and it would
silently undo the sandbox.

The default in `build_config` stays `network_mode: "none"`, with the grant as
one explicit branch, so a bug anywhere else in the granting path cannot open the
sandbox by accident.

## Consequences

**A task can resolve dependencies**, which is the point.

**Two containers and one network per attempt** instead of one container. The
mediator is small and short-lived, and teardown is attempted on every path the
sandbox can take. A survivor is a bug, not a race.

**DNS still resolves inside the sandbox.** Docker's embedded resolver runs in
the container's namespace and forwards through the daemon, so a name lookup can
succeed on an internal network even though no packet can follow it. This is
harmless — resolution is not access — and it is why the adversarial tests assert
at the TCP level rather than on `getent`.

**The receipt gains two fields.** `network_policy` records what was asked for;
`effective_egress` and `egress_mediator` record the `host:port` set that
survived grant intersection and expansion, and the image that enforced it. "It
could only reach the registry" is not a checkable claim otherwise.

**Nothing issues a capability grant yet.** The planning path does not decide
that a task needs npm, so `effective_egress` records an empty set in practice.
The enforcement is complete and unused; issuing grants is the next change, and
it is a planning decision rather than a boundary one.

**Registry host sets drift.** Ecosystems move CDN hosts, and a stale table looks
to a customer like a broken build. The table is the right place for the
decision, but it needs an owner and a review cadence.

**This is container-specific.** The microVM phase will need its own mediator
attachment, most likely over vsock. The `SandboxRunner` trait still carries no
container vocabulary, so that stays a swap.

## Alternatives rejected

- **Attach a normal Docker network and filter in the proxy.** Rejected: the
  sandbox would have a route out, and the filter would be optional from the
  task's point of view.
- **A shared mediator for all attempts.** Rejected: one task's allowlist would
  be serving another task's traffic, and the process would be a cross-tenant
  boundary maintained by a hash map.
- **Terminate TLS to inspect requests.** Rejected: it puts every customer's
  registry traffic in plaintext inside a component whose only job is to say yes
  or no to a hostname it already has.
