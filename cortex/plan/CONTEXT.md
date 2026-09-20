# The context engine

> Decision record. Written 2026-08-05 against `origin/main` after PR #439.
> Companion to [VERIFIER.md](VERIFIER.md) — verification is what Cortex sells;
> context is what makes the work worth verifying. Phase 2.5+ of
> [PLAN-2026-08.md](PLAN-2026-08.md), promoted to a named workstream.

---

## Why this document exists

The enterprise coherence claim ("200 developers working simultaneously without
stepping on each other") decomposes into two halves:

1. **Enforcement** — stopping two runs from clobbering the same surface.
   Cortex has this: resource leases with conflict detection
   (`crates/api/src/db.rs` — `find_resource_lease_conflict_tx`,
   `ResourceLeaseConflict`) and a `repo_scope` on run creation
   (`crates/api/src/routes.rs`).
2. **Understanding** — knowing *which* surface a task will actually touch,
   and giving the model the right slice of a large codebase. Cortex has
   **none of this.** Verified 2026-08-05: no tree-sitter, no symbol index, no
   embeddings, no repo map anywhere in `crates/`. The decomposer passes along
   whatever `file_paths` the user typed (`crates/engine/src/decomposer.rs:12`).

Competitors have made half 2 the category battleground — semantic codebase
indexing with cross-service dependency tracing is the top-ranked
differentiator in enterprise evaluations (discounting appropriately for
vendor-authored rankings, the capability gap they describe is real). A harness
that routes brilliantly and verifies rigorously but reads the repo like a
grep session loses the enterprise conversation in the first demo.

## The decision

**Cortex gets a repo-local context engine built from three signals — lexical,
structural, semantic — added in that order, stored in boring relational
tables, serving four internal consumers before any external one.**

And its strategic frame, stated so nobody over-invests: context is Cortex's
**second pillar, built to credibility, not supremacy**. The incumbent has
years of indexing engineering; Cortex will not out-index them in 2026. It
does not need to: the wedge remains *verified outcomes at half the frontier
bill across every frontier model*. The context engine must be good enough
that the wedge is believable on a real monorepo — roughly 70% of the
practical value comes from the first two signals, which are weeks of work,
not years.

---

## The three signals

### 1. Lexical (exists, formalize it)

ripgrep-grade text search over the working tree. Already implicitly available
to CLI workers; the engine wraps it as a ranked query primitive so planning
steps can call it deterministically. Zero new infrastructure.

### 2. Structural — the symbol graph (the real work)

Tree-sitter parsing (mature Rust crates, grammars for every language in the
repo's stack) producing four edge types into SQLite:

```sql
-- All tables live in .cortex/context-index.sqlite — a DERIVED CACHE, not
-- application state. Own file, own schema_tag, dropped and rebuilt on
-- mismatch. It deliberately does NOT touch the main DB's single-counter
-- migration chain (see #437/#438 ordering constraint — this cache opts out
-- of that entire class of problem).

CREATE TABLE symbols (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL,          -- fn | struct | class | export | ...
    file        TEXT NOT NULL,
    line        INTEGER NOT NULL,
    lang        TEXT NOT NULL
);
CREATE TABLE symbol_edges (
    src         INTEGER NOT NULL REFERENCES symbols(id),
    dst         INTEGER NOT NULL REFERENCES symbols(id),
    kind        TEXT NOT NULL           -- defines | references | imports | calls
);
CREATE TABLE file_meta (
    file        TEXT PRIMARY KEY,
    tree_hash   TEXT NOT NULL,          -- content hash at index time
    indexed_at  INTEGER NOT NULL
);
```

Not a graph database. Edges in relational rows, traversed with recursive CTEs
and bounded depth. A graph DB is operational surface without evidence of
need — if bounded-depth CTEs ever become the bottleneck at real scale, that
is a good problem recorded here for reconsideration.

**Incremental by construction:** `file_meta.tree_hash` makes staleness
detectable per file; a worker's diff invalidates exactly the files it
touched. An index pinned to a tree hash is part of run evidence — planning
against a stale index is a context bug with a name, not a mystery.

### 3. Semantic — embeddings (deferred, deliberately)

Embedding retrieval is deferred until the first two signals have a measured
recall gap. Reasons: hybrid lexical+structural covers most practical queries
in code; embeddings add per-index COGS and a provider dependency; and an
embedding index of customer code is a data-handling surface Cortex should not
grow before the enterprise story needs it. When it comes, it is a fourth
table in the same cache file, never a service.

### The repo map (first deliverable, days not weeks)

Aider-proven, cheap, immediately valuable: a ranked one-page skeleton of the
repo — top symbols per file, weighted by reference count — prepended to every
planning prompt under the existing token budgets
(`crates/core/src/evaluator.rs` — `token_budget`). This alone moves plan
quality on large repos more than any router change would.

---

## Four internal consumers

The engine is a crate (`crates/context`) behind one trait, and these four
call sites are its definition of done — not a demo search box:

| Consumer | Call | Effect |
|---|---|---|
| Decomposer (`crates/engine/src/decomposer.rs`) | expand user `file_paths` → impact-aware seed set | plans stop being blind to unnamed dependencies |
| Captain / step context packing | rank + pack retrieved slices under `token_budget` | steps see the *right* 500k tokens, not the nearest ones |
| Verifier check derivation (VERIFIER.md §check-derivation) | ecosystem + ownership detection from the index, not path sniffing | required checks derived from what the tree *is* |
| **Semantic leases** (the coherence unlock) | impact set = bounded closure over `symbol_edges` from the step's target files | leases claim what a change *actually touches* |

The fourth is the enterprise answer. Today a lease claims the paths someone
thought to list. With the graph, a run claims the bounded dependency closure
of its edit surface — and two runs editing different files that share a
dependency conflict *before* they collide in a merge, with the graph path as
the human-readable explanation ("run A holds `validateToken` via
`checkout-service` → `auth-lib`"). The existing
`ResourceLeaseConflict` machinery stays as the enforcement point; the graph
upgrades what gets claimed. That is the Augment-Intent-style coherence story,
grounded in machinery Cortex already has.

## External surface (later)

One MCP server exposing search/definition/references/impact-set, so external
clients (including competitors' own tools) can consume the index. This is
distribution, not architecture — it ships only after the four internal
consumers are real.

---

## What not to do

- No graph database. No vector database as a service. Boring rows, one cache
  file, rebuildable from a checkout at any time.
- No LLM-written summary index as a primary signal — summaries drift from
  code silently; the graph cannot.
- No whole-repo embeddings on day one, and no customer-code embeddings
  before the data-handling posture is written down.
- No indexing service that runs anywhere except next to the repo checkout.
- Do not block the verifier workstream on this — VERIFIER.md V1–V7 ships
  first; this engine's verifier hook (row 3 above) upgrades check derivation
  when it arrives.

---

## Build plan (hand-off)

| # | Task | Effort |
|---|---|---|
| C1 | Repo map: tree-sitter symbol extraction + reference-count ranking + prompt injection under `token_budget` | Opus 5 · high |
| C2 | Symbol graph: full `symbols`/`symbol_edges`/`file_meta` index, incremental invalidation, `.cortex/context-index.sqlite` lifecycle | Opus 5 · high |
| C3 | Retrieval + packing: hybrid lexical/structural query API; captain packs step context through it | Opus 5 · high |
| C4 | Semantic leases: impact-set closure (bounded depth), lease claims move from path lists to computed sets, conflict messages carry the graph path | Opus 5 · xhigh |
| C5 | MCP surface for the index | Sonnet 5 · medium |

Sequencing: C1 is standalone and ships first. C2→C3→C4 in order. C4 is the
enterprise demo. Embeddings get a number only after C3 has a measured recall
gap to cite.

---

## Amendments — 2026-08-06 (grounding in [RESEARCH-2026-08.md](RESEARCH-2026-08.md))

**A1 — C3 packing is a correctness requirement, not an optimization.** The
research pass upgrades C3's status: long-context degradation begins well
before window limits (Chroma "Context Rot"; lost-in-the-middle), and
scoped-per-step contexts are the strongest reliability lever in the 2025–26
agent literature (−39% multi-turn penalty when state accumulates,
arXiv 2505.06120). Two concrete additions to C3's scope: (a) a per-attempt
context budget with mandatory compaction at the boundary, and (b) **packed
context size recorded in run evidence** — a step that needed an outsized
context is both a margin problem and a quality risk, and correlating verdicts
against context size is a dataset only a product with verdicts can build.

**A2 — Plan lint, a future fifth consumer (after C4; advisory only).** Between
decompose and execute: deterministic structural rules first (a leaf with an
empty check union not labeled UNVERIFIED; a leaf whose impact set exceeds its
lease claim; a DAG edge with no data dependency), then at most one
fresh-context model pass flagging specification-class defects — ambiguity,
missing constraints, the 41.8% top failure class in production multi-agent
systems (MAST, arXiv 2503.13657). Output is *annotations* on the plan,
never a block — models propose, never grade, including about plans.
