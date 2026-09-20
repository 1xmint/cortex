# Round 4 adversarial pass — working state

Resume file for the fourth adversarial review of
[HARNESS-EXCELLENCE-PLAN-2026-08.md](HARNESS-EXCELLENCE-PLAN-2026-08.md).
Delete this file when the round is closed out.

**Method:** rounds 1–3 attacked by product area and therefore kept finding the
same *class* of gap. Round 4 attacks along four different axes: adversarial
(what does optimisation pressure do to this design), capability (does the plan
actually beat one model, or only cost less than one), foundation (is the code
under the plan worth building on), and actuarial (does the guarantee survive
contact with customers who select against it).

## Findings, ranked

| # | Finding | Where it lands | Status |
|---|---|---|---|
| 1 | The agent writes the code *and* the tests the verdict is computed from. Frozen specs freeze the argv, not the exam. | Phase 27 | written |
| 2 | Best-of-N racing is a false-accept generator on the hard tail, and Phase 6.7 calls it the risk-free knob. | Phase 27.4 | written |
| 3 | Verifier precision is never measured, and every economic claim in 26 phases depends on it. | Phase 27.3 | written |
| 4 | The plan optimises cheaper-than-one-model; the vision is better-than-any-model. No mechanism raises the ceiling. | Phase 28 | written |
| 5 | Repo comprehension: `crates/context` is real and wired, but degrades **silently**, covers 4 grammars, leaves retrieval/impact off the dispatch path, and keeps nothing between runs. | Phase 29 | written |
| 6 | Nothing evaluates Cortex. Champion/challenger grades routes, not the harness. | Phase 30 | written |
| 7 | Refund-on-failure is an insurance product with no actuarial layer, no right to decline, and no revenue-recognition treatment. | Phase 31 | written |
| 8 | Human review capacity is the binding constraint on delivered value and is unmodelled. | Phase 31.6 | written |
| 9 | No kill switch, no circuit breaker, no compensation for external side effects, no gate on agent-added dependencies. | Phase 32 | written |
| 10 | Solo-to-200-developer range: merge queues, stacked changes, base drift, and conflict awareness against open human work all appeared zero times. | Phase 33 | written |
| 11 | Foundation score 4.5/10; `crates/api` holds two products behind one poisoning mutex. | Phase 34 | written |

## Commit trail

Each phase lands as its own commit so an interrupted session loses at most one.

- `27` verification under adversarial pressure — done
- `28` capability above the frontier model — done
- `29` repository comprehension — done
- `30` Cortex evaluates Cortex — done
- `31` the economics of a guarantee — done
- `32` operational controls — done
- `33` solo-to-200-developer range — done
- `34` foundation assessment — done
- housekeeping: invariants 24–31, the Phase 6.7 racing amendment, delivery
  order PRs AL–AT, ten acceptance rows, completeness claim, sources — done

Round 4 is closed. The doc is 6,973 lines, 34 phases, 31 invariants.
A fifth round should attack an axis none of the first four used — candidates
are listed in Phase 34.4.
