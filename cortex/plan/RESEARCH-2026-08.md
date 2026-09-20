# Cortex Research Brief — Competitive Landscape, Top-Down Engineering, Multi-Agent Coherence

> **Research input, not a decision record.** Written 2026-08-06 against the
> canonical plan docs (VERIFIER.md, SURFACE.md, CONTEXT.md, PACKAGING.md,
> CREDITS.md, PLAN-2026-08.md). Nothing here changes a decision by itself:
> pricing and positioning moves are Josh's; engineering recommendations enter
> through amendments to the decision docs they name. Web claims mix first-party
> sources (GitHub changelog, arXiv, Kiro docs, OpenAI, Cognition) with
> third-party 2026 roundups; where a load-bearing claim rests only on a
> third-party aggregator it is flagged. Sources at the end.

---

## 1. Competitive landscape, August 2026

### The single most important structural fact

**In H1 2026 the entire category moved TOWARD effort metering, not away from it.**
Three repricings in four months, all in the same direction:

- **GitHub Copilot** — June 1, 2026: flat Premium Request Units replaced by
  token-metered "AI Credits" (1 credit = $0.01). The coding agent had briefly been
  the category's most outcome-shaped price (July 2025: one premium request per
  session regardless of complexity — a real per-task price) and GitHub **retreated
  from it** to token metering. Legacy Opus-class multipliers reportedly jumped
  7.5×→27× (third-party sourced; the 1-request-per-session fact is first-party).
- **OpenAI Codex** — April 2, 2026: token-based credit billing on Plus/Pro/Business,
  extended to Enterprise April 23. Credits refill on a rolling 5-hour window; cloud
  sessions add container fees (~$0.03–$1.92 per 20-min session by machine size).
  A "task" costs a variable, unpredictable 5–45 credits.
- **Cursor** — March 2026 restructure: background agents bill **per compute-minute
  on top of token costs**. Plans $0/$20/$60/$200 + Teams $40/seat.

Everyone is now selling *effort* (tokens, minutes, attempts, ACUs). The customer
carries all variance risk. "Bill anxiety" (SURFACE.md's phrase) is no longer a
hypothesis — it is the dominant complaint in every 2026 pricing writeup
("agentic bill shock" is a literal article title). **Cortex's fixed-price-per-
verified-task with refund-on-failure is counter-positioned against the whole
category's 2026 direction, and none of the incumbents can follow without
abandoning their meter** — their COGS story depends on passing variance through.

### Per-competitor: verification / pricing / receipt UX

| Product | (a) Verification & trust | (b) Pricing model | (c) Review/receipt UX when done |
|---|---|---|---|
| **Devin (Cognition)** | Red/orange/green *confidence estimate* per ticket (a model opinion, not executed evidence). Devin Review / "Bug Catcher" labels findings by confidence + CWE severity. Reports 67% PR merge rate (up from 34% YoY), 89% of its own commits | ACUs — effort units (~15 min of work): Core $20/mo + $2.25/ACU; Team $500/mo incl. 250 ACUs, extra at $2. Pay whether or not the task succeeds | PR + session link + confidence colors. No independent re-execution, no charge/receipt linkage |
| **Cursor background agents** | None beyond CI the customer already has; Bugbot (paid add-on) is an LLM reviewer — an opinion layer | Seats + usage + per-compute-minute for background agents (Mar 2026) | Agent finishes → PR appears. Up to 8 parallel agents. No evidence artifact at all |
| **GitHub Copilot coding agent** | **Process transparency, not outcome proof**: session logs; since Mar 2026 an `Agent-Logs-Url` commit trailer traces any commit to its session; enterprise session streaming/REST audit records (prompts, responses, tool calls) in preview | AI Credits (token-metered) since June 2026; Pro $10 (incl. $15 credits), Pro+ $39 ($70), Max $100 ($200) | Draft PR + "view session" log. Logs show what the agent *did*; nothing independently proves checks *passed*. Agent HQ "mission control" spans Copilot/Claude/Codex/Devin/Grok agents on one dashboard |
| **Factory.ai** | Droids run tests but self-report; enterprise trust story is compliance-shaped (ZDR, on-prem, audit logs, partitioned inference), not proof-shaped. $1.5B valuation, Series C Apr 2026; NVIDIA/Adobe/EY/Adyen | Pro $20 / Plus $100 / Max $200, token-based standard rates; custom enterprise | PR + session trace. No receipts |
| **Codegen** | LLM self-review loops; no independent gate | Usage-based API pricing (token pass-through economics) | PR + logs |
| **Google Jules** | Plan shown before execution (approve/edit); runs tests in its VM and self-reports results | **Task-count quotas** — the closest thing to per-task pricing, but a "task" = an *attempt* (a plan revision burns a second task). Free 15/day; AI Pro $19.99 100/day; Ultra $124.99 300/day. Explicitly **not pooled** across a team | Plan diff + audio changelog + PR. Attempts are billed even when they fail |
| **OpenAI Codex cloud** | **Best-in-class evidence *citation*** — every task ships citations of terminal logs and test outputs so you can trace each step. But it is the worker citing its *own* sandbox — self-reported evidence, exactly what VERIFIER.md's trust model demotes to "hints" | Token credits (Apr 2026), 5-hr rolling windows, container fees | Diff + traceable logs. The category's high-water mark for receipts, and still fails Cortex's independence test |
| **Amazon Kiro** | **The real threat.** Spec-driven: requirements.md (EARS-format acceptance criteria) → design.md → tasks.md; GA Mar 2026; replaced Amazon Q May 2026. GA release added **property-based testing** that checks generated code against the spec — deterministic verification, "not another LLM's opinion" (their framing, nearly verbatim Cortex's) | Request-counting: Pro $20 (225 vibe + 125 spec requests), Pro+ $40, Pro Max $100, Power $200; overage credits $0.04. **Charged per request whether or not the spec check passes** | In-IDE spec workflow + task list + PBT results. No forge-native delivery, no billing linkage, verification runs in the user's environment |

### What NOBODY does — the exploitable gaps

1. **Nobody bills on a verified outcome.** ACUs, tokens, minutes, request counts,
   task-*attempts* — all effort meters. Deloitte published accounting guidance for
   outcome-based pricing in agentic AI (June 4, 2026), and per-resolution pricing
   is normal in customer-service agents ($0.50–$2.00/resolution) — the model is
   legible to CFOs and auditors, and **zero coding-agent vendors use it**.
2. **Nobody refunds failed work.** Jules charges for failed attempts; Devin burns
   ACUs on failures; Copilot/Codex burn credits. A refund-on-FAILED promise is
   unanswerable by any incumbent without a verifier they don't have.
3. **Nobody produces an independently re-executed receipt.** The 2026 frontier is
   Codex's self-citations and Copilot's commit→session-log trailer — both are the
   worker narrating itself. Kiro has deterministic checks but in the user's IDE,
   unpinned, unlinked to money. Cortex's receipt (independent re-execution, pinned
   runner image, replayable evidence, charge and refund keyed to the verdict id) has
   **no equivalent anywhere**.
4. **Nobody reconciles the invoice.** PACKAGING.md's line — a CFO can reconcile the
   invoice against verification receipts line by line — is genuinely unique.
   Copilot's enterprise session-streaming preview shows demand for auditability;
   it audits *process*, not *charges*.
5. **Nobody pools outcome-priced credits at the org level.** Jules is explicitly
   per-user, non-pooled; Cursor Teams is per-seat; procurement complaints about
   paying for unused allowances are the most-cited in the category.
6. **Watch-item, not a gap:** GitHub Agent HQ already ships a "mission control"
   dashboard across multi-vendor agents and the name collides with SURFACE.md's
   language. Cortex's six-pane app must be differentiated as *receipts + ledger +
   leases* (things Agent HQ cannot show), not "see your agents running" (commodity
   by mid-2026).

---

## 2. Top-down engineering as a Cortex capability

### State of the art, August 2026

- **Kiro** is the mainstream implementation: 3-artifact flow (EARS-format
  requirements → design doc surfacing trade-offs → sequenced task list), agents
  execute tasks in parallel where the plan allows, and (since GA) property-based
  tests verify code-matches-spec. Weakness: per-feature, in-IDE, single-dev scope;
  the spec is not priced, not approved as a unit, not bound to billing.
- **Copilot Workspace** (the original brainstorm→spec→plan→implement product) was
  **sunset May 30, 2025**; its planning step was folded into the coding agent as a
  lightweight preamble. GitHub effectively abandoned standalone plan-as-artifact.
- **Tessl** bets on spec-as-source-of-truth (specs live in repo as long-term
  memory; capabilities with linked tests), plus a 10k+ spec registry for library
  usage. Framework still not GA after ~9 months of closed beta — signal that
  spec-first *authoring* is a hard sell; spec-first *verification* (Kiro's move)
  lands better. Martin Fowler's series (Kiro / spec-kit / Tessl) is the canonical
  taxonomy.
- **Research:** the field converged on exactly the shape Cortex needs:
  - **VeriMAP** (arXiv 2510.17109, EACL 2026): a planner that decomposes a task
    into a DAG **and emits a per-subtask verification function** (executable
    Python or NL criteria) at plan time; executors work scoped subtasks; verifiers
    gate; coordinator retries/replans. Outperforms single- and multi-agent
    baselines and improves interpretability. This is academic validation of
    VERIFIER.md's "contract checks frozen at plan time" — extended to *every node
    of the DAG*.
  - Subgoal-DAG planning with post-conditions per node, and scoped per-subgoal
    contexts isolating each leaf from the global transcript (matches CONTEXT.md
    C3 packing).
  - **MAST** (arXiv 2503.13657, NeurIPS 2025; 1,600+ annotated traces): the #1
    failure class in multi-agent systems is **specification problems (41.8%)** —
    ambiguous roles, unclear task definitions, missing constraints — ahead of
    coordination (36.9%) and verification gaps (21.3%). Decomposition quality is
    the top production failure source; verification-aware planning attacks it
    directly.
  - **The Verification Horizon** (Qwen, arXiv 2606.26300): every verifier is a
    proxy for intent; under optimization pressure the proxy-intent gap widens
    (reward hacking / teaching-to-the-test); no fixed reward stays effective as
    generator capability grows — verification must co-evolve. Direct warning for
    frozen check sets that workers can see.

### Could "verified decomposition" be a Cortex product? Yes — and it is nearly free

Cortex already has every ingredient: a decomposer (`crates/engine/src/decomposer.rs`),
plan-time contract checks (VERIFIER.md §check derivation), per-leaf UNVERIFIED
labeling *at plan time*, per-class credit pricing quoted before run (SURFACE.md
cost confidence), and the plan "visible as a checklist on the issue" (SURFACE.md
drop-in path step 4). What no doc yet names is the composite artifact:

**The Plan Receipt** — a decomposition where every leaf carries (a) its derived
required checks, (b) its VERIFIED/UNVERIFIED label, (c) its credit price, and
(d) its impact set / lease claim (CONTEXT.md C4), delivered for approval *before
execution*, and archived as evidence afterward. The plan itself becomes an
auditable, priced, approvable object — reviewed like a PR.

Nobody sells this. Kiro's tasks.md is the nearest thing and it has no checks, no
prices, no leases, no approval gate, no archive. It also directly answers the
ByteByteGo-style top-down instinct: requirements → architecture → components →
implementation, except each edge of the decomposition is load-bearing because the
checks and the money attach to it.

Two cautions from the literature:

1. **Decomposition is where systems fail most** (MAST 41.8%) — so the plan needs
   its own quality gate (see rec 5), and plan revision must be cheap (Jules users
   hate that a plan revision burns a task; Cortex should never charge for
   re-planning).
2. **Frozen, worker-visible checks invite teaching-to-the-test** (Verification
   Horizon). Mitigation exists and has a competitor precedent: property-based /
   generated check variants (Kiro's PBT) are far harder to overfit than example
   tests. See rec 2 (held-out checks).

---

## 3. Multi-agent coherence: the founder's observation vs. the research

**The observation — "two models conversing drift and degrade, but a fresh model
reviewing the transcript instantly regains coherence" — is strongly supported,
with one critical asymmetry the research adds.**

What the literature actually shows:

- **Multi-turn degradation is real and large.** "LLMs Get Lost in Multi-Turn
  Conversation" (Microsoft/Salesforce, arXiv 2505.06120; 200k+ simulated
  conversations, 15 models): **−39% average** vs single-turn across six generation
  tasks. Mechanism: models make premature assumptions in early turns, over-commit,
  and *do not recover* — "when LLMs take a wrong turn, they get lost." Degradation
  decomposes into small aptitude loss + **large unreliability increase**. Crucially,
  consolidating the scattered conversation into a single fresh prompt restores most
  of the lost performance — the information was sufficient; the *accumulated
  transcript state* was the poison. That is the founder's observation, measured.
- **Long context degrades even without conversation.** Chroma's "Context Rot"
  (July 2025, 18 frontier models): accuracy drops non-uniformly as input grows,
  sometimes 30–50% well before the window limit. Lost-in-the-middle (Liu et al.):
  U-shaped attention; >30% drops for mid-context information. A long transcript is
  a degraded substrate *per se*.
- **Two-model conversations specifically decay.** Multi-agent debate research:
  sycophancy collapses debates into premature consensus — disagreement rate falls
  as debate progresses, and that fall correlates with performance degradation
  ("Peacemaker or Troublemaker," OpenReview; "Talk Isn't Always Cheap," arXiv
  2509.05396). "Degeneration-of-Thought" (Liang et al.): once confident, a model
  cannot generate novel corrective thoughts through its own reflection. "Problem
  drift" (arXiv 2502.19559): generative tasks drift off-task in **76–89%** of long
  debates (causes: lack of progress 35%, low-quality feedback 26%, lack of clarity
  25%). MAST puts inter-agent misalignment at 36.9% of production failures.
- **The critical asymmetry: fresh eyes alone are not enough.** Huang et al. (ICLR
  2024, arXiv 2310.01798): *intrinsic* self-correction — a model re-examining its
  own output without external signal — fails and often degrades performance;
  reported gains in the literature came from oracle labels. Weaver (NeurIPS 2025)
  measures a persistent gap between LM judges and oracle (execution-grade)
  verifiers. So: a fresh-context *reader* recovers coherence (transcript
  consolidation result), but a fresh-context *grader* is only reliable when it
  holds an independent ground signal — execution, tests, type checks. Fresh
  context fixes *state contamination*; it does not fix *judgment*.
- **Industry synthesis matches:** Cognition's "Don't Build Multi-Agents" (June
  2025) — single-threaded writer with continuous context + a dedicated
  compressor model; by 2026 the converged pattern is one orchestrator that owns
  context and spawns **ephemeral, read-only, fresh-context subagents returning
  compressed summaries**. Parallel writer swarms remain fragile.

### Product mapping — how far to extend the fresh-context-evaluator pattern

Cortex's verifier is the *strongest possible* instance of the pattern: fresh
environment + fresh tree + deterministic execution = fresh context **with** ground
truth. The research says extend the pattern where a ground signal or a
consolidation benefit exists, and refuse it where it would reintroduce opinion:

| Extension | Verdict | Grounding |
|---|---|---|
| **Fresh-context retry after FAILED** — never continue the failed worker's conversation; new attempt gets contract + receipt tails + packed context, zero transcript | **Do it. Highest-value, near-zero cost.** | 2505.06120 (no recovery from wrong turns); Huang (self-correction degrades); the consolidation result |
| **Plan lint — fresh-context review of the plan DAG before execution** | Do it, as *advisory annotations + deterministic structural rules*, never a verdict | MAST (spec failures 41.8%); VeriMAP (verification-aware planning wins); DRIFTJudge precedent |
| **Per-step scoped context packing (C3) instead of inherited transcript** | Already planned — the research upgrades it from optimization to correctness requirement | Context rot; lost-in-the-middle; TDP scoped contexts |
| **Compressor checkpoint for long runs** — consolidate run state into a fresh summary at phase boundaries rather than letting the captain's context grow | Do it when runs exceed a measured turn budget | Cognition compressor; 39% multi-turn penalty; context rot curves |
| **Evaluator gates (LLM) between agent phases grading quality** | **Refuse.** Keeps VERIFIER.md's line: models propose, never grade | Weaver's judge-oracle gap; Huang; sycophancy results |
| **Multi-model debate/consensus layers anywhere** | **Refuse.** | Premature-consensus collapse; problem drift 76–89%; DoT |

One reframe worth writing down: **context resets are not a workaround, they are
an architectural principle with a measured effect size (~39%).** Cortex's
step-shaped execution (each leaf a fresh worker with packed context, each verdict
a fresh sandbox) is accidentally aligned with the strongest finding in the 2025–26
agent-reliability literature. Name it in the docs and defend it in design review —
"no step inherits a transcript" is as important a rule as "no model grades a check."

---

## 4. Concrete recommendations (prioritized)

1. **[PACKAGING.md + VISION] Position against the H1-2026 repricing wave, by name.**
   Copilot→AI Credits (June 2026), Codex→tokens (April 2026), Cursor→compute-minutes
   (March 2026): the whole category converted to effort metering within four months.
   Marketing artifact: a "your invoice, reconciled" page — every line item links a
   charge to a replayable receipt. No competitor can copy this without abandoning
   their meter. This is a timing window: bill-shock articles are the category's
   dominant press *right now*.

2. **[VERIFIER.md — amend §check derivation] Split required checks into
   worker-visible and held-out.** Freeze all checks at plan time (unchanged), but
   withhold a derived subset from the worker's prompt — property-based or
   metamorphic variants of the visible contract checks (Kiro's PBT proves the
   technique is productizable; The Verification Horizon documents why visible
   frozen checks degrade under optimization pressure). The worker sees *what must
   be true*; it does not see *every probe*. Ships as a small extension of V2.

3. **[NEW doc: PLAN-RECEIPT.md; amends SURFACE.md F3 and VERIFIER.md] Ship
   "verified decomposition" as a sellable artifact.** Plan DAG posted to the issue
   as a checklist where every leaf shows: derived checks, VERIFIED/UNVERIFIED
   label, credit price, lease/impact claim. Human approves the plan like a PR;
   execution starts on approval; the plan is archived with the run evidence.
   VeriMAP is the academic blueprint (per-subtask verification functions emitted
   by the planner). Kiro's tasks.md is the nearest competitor and has no checks,
   prices, leases, or approval gate. Re-planning is always free (Jules's most
   hated behavior is charging for plan revisions).

4. **[VERIFIER.md V4 / ARCHITECTURE heal path] Codify the fresh-context retry
   rule: a FAILED attempt's transcript is never continued.** Retry = new worker,
   fresh context, packed inputs = contract + receipt tails (failed check output)
   + C3 slices. Measured grounding: models do not recover from wrong turns
   (−39%, arXiv 2505.06120); self-correction without external signal degrades
   (ICLR 2024). The receipt tail *is* the external signal — Cortex is uniquely
   positioned to make retries converge because its feedback is executed evidence,
   not opinion.

5. **[CONTEXT.md — new consumer row] Plan lint: a deterministic + advisory gate
   between decompose and execute.** Deterministic rules first (leaf with empty
   check union not labeled UNVERIFIED; leaf whose impact set exceeds its lease;
   DAG edge with no data dependency), then one fresh-context model pass flagging
   spec-class defects (ambiguity, missing constraints — MAST's 41.8% class).
   Output is annotations on the Plan Receipt, never a block. Keeps "models
   propose, never grade" intact.

6. **[VERIFIER.md — metrics section, new] Instrument UNVERIFIED-rate from day
   one, per task class and per ecosystem.** The billing-honesty story collapses if
   half of real tasks fall out of check derivation. This number decides where
   check-derivation investment goes next (e.g., docs-class tasks may need
   rubric→property generators later) and it gates which task classes the refund
   promise is marketed on. Cheap: one column on `verification_runs` rollups.

7. **[CONTEXT.md C3 — amend] Per-attempt context budget with mandatory
   compaction, and record packed-context size in run evidence.** Context rot
   shows degradation begins well before window limits; a step that needed 400k
   tokens of context is both a margin problem and a quality risk. Logging it per
   attempt turns "context bloat" into a measurable regression, and correlating
   verdicts against context size is a dataset no competitor has (they don't have
   verdicts).

8. **[SURFACE.md — amend Receipts pane] Make receipts shareable, public-URL
   artifacts.** A receipt page (verdict, checks, pinned runner digest, tree hash)
   that a developer can link in a PR discussion or a CFO in an audit is the viral
   unit of the trust story — analogous to a cert-transparency entry. Codex's
   self-citations are the category's best evidence UX; a *third-party-replayable*
   receipt is categorically stronger and costs only a public read view of V6.

9. **[SURFACE.md — competitive posture note] Treat Kiro, not Devin, as the
   thesis-adjacent threat; interoperate with its spec format.** Kiro has
   deterministic spec verification (PBT), AWS distribution, and replaced Amazon Q
   — but no forge delivery, no billing linkage, no controlled-environment
   execution. Two moves: (a) accelerate F3/F4 before Kiro grows a GitHub App;
   (b) accept EARS-format acceptance criteria as contract-check input, so teams
   already writing Kiro-style specs can hand them to Cortex unchanged — their
   spec, our sandbox, our receipt.

10. **[SURFACE.md — amend ranking table] Decide the Agent HQ posture explicitly.**
    GitHub Agent HQ ships multi-vendor "mission control" (Copilot, Claude, Codex,
    Devin, Grok slots) to every paid Copilot org. It commoditizes "watch your
    agents run" — so cortex.heyvera.org must lead with what Agent HQ structurally
    cannot render: receipts, the ledger, and leases. Evaluate listing Cortex as an
    Agent HQ agent for distribution (check-run receipts render natively in their
    UI); the risk is meter mismatch (their billing rails assume premium
    requests/credits), which is also exactly the differentiation.

---

## Sources

Competitive landscape:
- Devin pricing/ACUs: https://www.lindy.ai/blog/devin-pricing ; https://pricepertoken.com/coding-assistants/devin ; https://aitoolpick.org/blog/devin-pricing-2026/
- Devin Review / confidence UX: https://docs.devin.ai/work-with-devin/devin-review ; https://research.contrary.com/company/cognition
- Cursor pricing/background agents: https://flexprice.io/blog/cursor-pricing-guide ; https://www.nocode.mba/articles/cursor-pricing ; https://aiproductivity.ai/blog/cursor-pricing/
- Copilot 1-premium-request-per-session: https://github.com/orgs/community/discussions/165798
- Copilot AI Credits June 2026: https://www.unerr.dev/blog/github-copilot-pricing-explained ; https://copilot-alternatives.com/blog/github-copilot-pricing-change-2026/ ; https://usagebox.com/articles/github-copilot-usage-based-billing-2026
- Copilot session-log trailer / audit streaming: https://github.blog/changelog/2026-03-20-trace-any-copilot-coding-agent-commit-to-its-session-logs/ ; https://github.blog/changelog/2026-03-19-more-visibility-into-copilot-coding-agent-sessions/ ; https://cyberogz.com/News/github-opens-copilot-agent-session-streaming
- Agent HQ: https://developers.slashdot.org/story/25/11/02/2337254/github-announces-agent-hq-letting-copilot-subscribers-run-and-manage-coding-agents-from-multiple-vendors ; https://www.digitalapplied.com/blog/github-agent-hq-multi-agent-platform
- Factory.ai: https://enterprisedna.co/resources/news/factory-ai-series-c-enterprise-coding-agents-2026/ ; https://www.xpay.sh/saas-pricing/factory-ai/
- Jules pricing/limits: https://hackup.ai/ai-plans/jules/ ; https://www.morphllm.com/comparisons/jules-google-coding-agent ; https://blog.google/innovation-and-ai/models-and-research/google-labs/jules/
- Codex pricing/billing: https://www.eesel.ai/blog/openai-codex-pricing ; https://uibakery.io/blog/openai-codex-pricing ; https://www.cloudzero.com/blog/openai-codex-pricing/
- Codex citations/evidence UX: https://openai.com/index/introducing-codex/ ; https://codegen.com/ai-tools/openai-codex/
- Kiro spec-driven + PBT: https://kiro.dev/docs/specs/correctness/ ; https://kiro.dev/blog/property-based-testing/ ; https://kiro.dev/changelog/spec-correctness-and-cli/ ; https://kiro.dev/blog/general-availability/ ; https://trendytechtribe.com/ai/amazon-kiro-free-year
- Outcome-based pricing accounting: https://dart.deloitte.com/USDART/home/publications/deloitte/industry/technology/accounting-outcome-based-pricing-agentic-ai ; https://pickaxe.co/post/ai-agent-pricing-models ; https://thepricingconundrum.substack.com/p/outcome-based-pricing-in-practice
- Per-task cost math: https://www.kunalganglani.com/blog/ai-agent-cost-per-task-2026

Top-down / spec-driven:
- Copilot Workspace sunset: https://www.qwe.edu.pl/tutorial/github-copilot-workspace-planning/ (secondary)
- Tessl: https://tessl.io/blog/tessl-launches-spec-driven-framework-and-registry/ ; https://docs.tessl.io/use/spec-driven-development-with-tessl ; https://codemyspec.com/blog/tessl-review
- Fowler on Kiro/spec-kit/Tessl: https://martinfowler.com/articles/exploring-gen-ai/sdd-3-tools.html
- VeriMAP: https://arxiv.org/abs/2510.17109 (EACL 2026: https://aclanthology.org/2026.eacl-long.353.pdf)
- Verification Horizon (Qwen): https://arxiv.org/abs/2606.26300
- Subgoal DAG / long-horizon planning: https://zylos.ai/research/2026-05-14-long-horizon-planning-goal-decomposition-ai-agents/ ; https://arxiv.org/html/2604.11378v1
- Executable acceptance criteria: https://www.braingrid.ai/blog/how-to-write-acceptance-criteria-ai-agent-can-verify

Multi-agent coherence:
- LLMs Get Lost in Multi-Turn Conversation: https://arxiv.org/abs/2505.06120 ; code: https://github.com/microsoft/lost_in_conversation
- Context Rot (Chroma): https://www.trychroma.com/research/context-rot
- Lost in the Middle (Liu et al.): summarized via https://www.morphllm.com/context-rot
- Self-correction fails without external signal: https://arxiv.org/abs/2310.01798 (ICLR 2024)
- Weaver / generation-verification gap: https://arxiv.org/html/2506.18203v1 ; https://neurips.cc/virtual/2025/poster/117007
- MAST failure taxonomy: https://arxiv.org/abs/2503.13657 (NeurIPS 2025)
- Problem drift in debate: https://arxiv.org/abs/2502.19559
- Sycophancy in debate: https://openreview.net/forum?id=hkBM5QkFVg ; https://arxiv.org/pdf/2509.05396
- Degeneration-of-Thought / MAD: https://arxiv.org/abs/2305.19118
- Cognition, Don't Build Multi-Agents: https://cognition.com/blog/dont-build-multi-agents

Reliability notes: Copilot's 27× Opus multiplier and the exact June-2026 credit
inclusion figures rest on third-party pricing trackers (unerr.dev,
copilot-alternatives.com, usagebox.com) — verify against GitHub's own billing docs
before quoting publicly. Kiro request-count splits (225/125) are from a
third-party tracker dated July 4, 2026. All arXiv/GitHub-changelog/Kiro-docs/
Cognition/OpenAI claims are first-party.
