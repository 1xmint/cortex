# Cortex Operations Room Reference

Status: canonical

This document is the current repo-truth contract for Cortex's batter-first product foundation. Raw brainstorms stay in `internal/`; accepted implementation work should preserve this boundary unless a proposal or ADR changes it.

## Product Shape

Cortex is an operations room for AI-assisted software work.

It has one underlying engine and several views:

- **Project Chat:** collaboration workbench for intent, judgment, debugging, review, and detailed implementation.
- **Task Manager:** operational control surface for intake, queueing, prioritization, dispatch, approval, retry, cancellation, and evidence inspection.
- **Live Map:** topology/debugging lens over real active work.
- **Personal Operations:** user-level inbox across authorized personal and org scopes.
- **Org Operations:** shared authority root for one team/company/GitHub organization.

## Canonical Graph

All views should project the same backend-owned graph:

```text
Scope -> Project -> Task -> ChatThread
                 -> Run -> Step -> Attempt -> Evidence -> VerifierReport
```

Supporting entities:

- `Ask`: human decision request.
- `Lease`: exclusive claim over a branch, path, issue, PR, migration, deployment, or other conflict-prone resource.
- `Authority`: the personal or org context under which a write is allowed.
- `AuthorityScope`: backend-owned personal, team, org, or future portfolio boundary with memberships, resources, and policy.

## Surface Ownership

| Surface | Owns | Does not own |
|---|---|---|
| Project Chat | conversation, intent, collaboration, human judgment | canonical run/task state |
| Task Manager | queue, priority, controls, blockers, approvals, inspector | private local production truth |
| Live Map | topology, blockers, dependencies, evidence/debug visibility | primary queue or decorative graph state |
| Personal Operations | authorized personal inbox and cross-scope attention | org policy bypass |
| Org Operations | shared org tasks, policy, audit, GitHub/repo work | private scratch work before promotion |

## Done Means Evidence-Backed

Cortex must not treat work as done only because an agent said it is done.

Task completion requires:

- a completed run
- terminal required steps
- captured evidence
- verifier acceptance or explicit human override
- no unresolved blocking Ask

Evidence should include, as applicable:

- changed files
- git diff or patch summary
- command logs
- test/build/lint results
- artifacts and receipts
- verifier verdict and diagnostic

## Status Vocabulary

Task:

```text
inbox -> ready -> active -> blocked -> review -> done
                           -> failed
                           -> cancelled
```

Run:

```text
planned -> queued -> running -> verifying -> completed
                              -> blocked
                              -> failed
                              -> cancelled
```

Step:

```text
pending -> ready -> leased -> running -> verifying -> completed
                                      -> blocked
                                      -> failed
                                      -> orphaned
                                      -> cancelled
```

## Implementation Rules

1. Prefer backend projections over local UI truth for production state.
2. Do not add fake seeded activity to production surfaces.
3. Attach chats to projects or tasks, not every run/step/retry.
4. Show why a new chat was created if Cortex creates one.
5. Treat Live Map as read-only until leases and authority checks exist.
6. Keep personal and org scopes on the same engine.
7. Require explicit org-context handoff before mutating org resources from Personal Operations.
8. Require run creation against non-personal repo resources to name the authority scope; if policy requires handoff, require a handoff id before dispatch.
9. Treat multi-org operations as read aggregation until single-org authority, handoff, audit, approval, and leases are enforced.
10. Add append-only events before making Task Manager the production source of truth.
11. Add resource leases before parallel autonomous writes.
12. Record verifier evidence before rewarding routing/provider decisions.

## V1 Batter Cut

Ship first:

- one Operations Room engine
- personal scope plus one org-shaped scope
- backend task/run/step/evidence projection
- task inspector
- Project Chat attachment to project/task
- verifier/evidence floor
- approval inbox
- basic resource leases for the highest-risk write surfaces
- read-only Live Map over real data

Defer until after foundation:

- multi-company executive cockpit
- rich animated map polish
- automatic parallel autonomy without leases
- full GitHub write automation
- cross-org spend optimization
- marketplace or agent roster surfaces

## Related Docs

- proposal: `docs/proposals/cortex-operations-room-contract.md`
- authority proposal: `docs/proposals/cortex-authority-and-scope-model.md`
- authority ADR: `docs/decisions/ADR-0008-cortex-authority-and-scope-boundaries.md`
- live map proposal: `docs/proposals/live-orchestration-mapping-view.md`
- backlog source: `internal/backlog/cortex-operations-room-foundation-checklist.md`
