# Round 5 adversarial pass — working state

Resume file for the fifth adversarial review of
[HARNESS-EXCELLENCE-PLAN-2026-08.md](HARNESS-EXCELLENCE-PLAN-2026-08.md).
Delete this file when the round is closed out.

**Opened:** 2026-08-12
**Base commit:** `a466bb1f` (main — "longform by handle (#529)")
**Branch:** `docs/round5-teaching`

## Axis

Rounds 1–4 attacked product area, then adversarial/capability/foundation/actuarial.
Round 4 closed by saying a fifth round should attack an axis none of the first
four used. This round attacks the **human** axis: not whether Cortex is correct,
but whether the person on the other end of it is left more capable or less.

That axis is where Josh's teaching-layer request lands, and it is also where
Phase 34.4's own admitted weakness lives ("whether a solo builder can genuinely
use this is untested").

## Primary hypothesis under test — stated so it can be disproved

> **H0: developers do not want to be taught.** A learning product aimed at people
> who came to ship is a well-known way to build something nobody opens twice.

This is Josh's instinct and it is the null. Phase 35 may only be written if the
evidence beats it. Enthusiasm is not evidence.

## Sub-questions

| # | Question | Status |
|---|---|---|
| 1 | Does anyone want this? Grounded, not reasoned from enthusiasm. | answered — 35.2 |
| 2 | Which named surfaces survive — study plans, quizzes, flashcards, papers, sandbox exercises? | answered — 35.3 |
| 3 | What is the interruption cost, and is "never interrupts" a design constraint? | answered — 35.5 |
| 4 | Does one design serve both the solo builder and the 200-person org, or is that wishful? | answered — 35.4 |
| 5 | Where does the derived-from-record rule bind — thin records, `UNVERIFIED`, `none` battery? | answered — 35.6 |
| 6 | What is uniquely Cortex's, and is it as strong as it sounds? | answered — 35.7 |

## Verdict on H0

**H0 survives in half and is refuted in half, and the half that breaks is the
half that decides the product.** Developers demonstrably do learn — 69% learned
a new language or technique in the last year, and 44% already use AI tools to do
it — but every resource at the top of that list is *pulled* in the moment
(documentation 68%, online resources 59%, Stack Overflow 51%). Nothing
curriculum-shaped appears. So: build the pull, never the push. A study plan, a
scheduled lesson, and a tutor page are all the push, and all three are cut.

The finding that actually forces the phase is a different one, and it is not a
growth argument. A randomised trial of 52 mostly-junior engineers found that
delegating to an assistant produced a **17% lower comprehension score** on code
the participant had written minutes earlier — and that the *mode* of use, not
the use, decided it. Cortex as specified across 34 phases is the delegation
mode, exactly. The teaching layer is therefore remediation of a harm Cortex
causes, not an upsell attached to it.

## Findings, ranked

| # | Finding | Where it lands | Status |
|---|---|---|---|
| 1 | Cortex is by construction the interaction mode measured to damage comprehension. Teaching is remediation, not an upsell. | Phase 35.1 | written |
| 2 | Developers want to learn and refuse to be taught. Pull survives; every push surface — study plans, scheduled lessons, a tutor page — is cut. A separate page is also a mode switch, which invariant 31 already forbids. | Phase 35.2 | written |
| 3 | The named surfaces do not survive equally. Flashcards are cut outright; quizzes demote from a teaching surface to a *measurement instrument*; faded sandbox exercises and the receipt-as-worked-example rank first. | Phase 35.3 | written |
| 4 | The range splits on **expertise, not headcount**, and the expertise-reversal effect makes over-guidance actively harmful rather than merely redundant. One design serves both ends only because Cortex can *measure* the level instead of asking for it. | Phase 35.4 | written |
| 5 | Interruption cost is quantified and "never interrupts" holds as a hard constraint. The cost is the reconstruction, not the seconds. | Phase 35.5 | written |
| 6 | The derived-from-record rule has a hole: a thin record is the **common** case, not the edge. `UNVERIFIED`, a `none` battery, and a degraded map are the states a teaching layer would most want to paper over. | Phase 35.6 → invariant 32 | written |
| 7 | The unique corpus is real but the cold start is on the wrong side: for day-one onboarding the history belongs to the **repository**, not the person. Phase 35 is therefore downstream of Phase 29 as well as of a Cortex that runs. | Phase 35.7 | written |
| 8 | How this dies: the static-analysis adoption literature is the exact failure mode — unsolicited, non-actionable, occasionally wrong notes attached to code. The derived-from-record rule is what avoids it. | Phase 35.8 | written |

## Recommendation to Josh

Build it — **Q2 2027, not next quarter** — with one exception carved out that
should not wait: the receipt already owes the developer a plain-language account
of what failed and why a check was chosen. That is receipt quality, it is
already implied by the Phase 33.5 disclosure tiers, and it is the entire
teaching layer's foundation. Everything else waits for a Cortex that runs
(wave 5), a comprehension artifact (PR AO), and a scoreboard (PR AP).

## Commit trail

Each section lands as its own commit so an interrupted session loses at most one.

- state file opened — done
- `35` the teaching layer, argued and bounded — done
- invariants 32–33, delivery order PR AU, test matrix row, sources — done
