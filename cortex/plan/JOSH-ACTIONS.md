# Josh's queue, compressed

> Written 2026-08-05, corrected twice the same day. Four items were parked as
> "blocked on Josh." Fair challenge received: most of each is research and
> drafting an agent can do, and that work is now done below.

## What Josh's role actually is

Two things, and agents own everything else:

1. **Being the first user.** Sign up, open the chat interface, start a run,
   watch a task come back, check the receipt, spend a credit, hit a wall.
   This is not QA theatre — it is the only signal that catches what tests
   cannot: the thing that is confusing, the step that takes too long, the
   screen that lies. No agent can supply it, and nothing else on this list
   matters if the product is unpleasant to use.
2. **Credentials and authorizations.** API keys, Tailscale, Stripe, the
   accounts that only an owner can create. Agents cannot mint these and
   should not hold them.

The residue — signatures, payments, mail under your name — attaches to
launch gates below, not to daily work.

**The dependency nobody should miss: (1) is currently impossible.** Production
runs the June 3 binary, deploys are frozen behind #411, and the surface a user
would meet is still the BYOK-era chat shell (SURFACE.md F1). So the sequence is
unfreeze → ship → amputate the old surface → *then* Josh uses it as a customer
would. Item 1 below is what starts that chain, which is why it is first.

**Target state, so nothing gets parked by accident: a complete product an
external user can sign up for and use.** Items below are ordered by when they
block that, not by how uncomfortable they are.

---

## 1. Unfreeze deploys — #411's host commands (~1 min of yours)

**Your part is the Tailscale authorization**, not the typing. Once the
agent's SSH access is authorized, it runs the sequence below itself. Only
the `/etc/cortex/cortex.env` line may bounce back to you if guardian's sudo
allowlist doesn't cover it — that single line is the entire fallback.

SSH to the VPS (`ssh guardian-vps-tail`), then, in order:

```bash
sudo mkdir -p /var/lib/cortex && sudo chown guardian:guardian /var/lib/cortex
sudo systemctl stop cortex
mv /home/guardian/claw-net/.cortex/cortex.db* /var/lib/cortex/
mv /home/guardian/claw-net/.cortex/ledger.jsonl /var/lib/cortex/
printf 'CORTEX_DB_PATH=/var/lib/cortex/cortex.db\nCORTEX_LEDGER_PATH=/var/lib/cortex/ledger.jsonl\n' | sudo tee -a /etc/cortex/cortex.env
sudo systemctl start cortex
```

Then on GitHub: open PR #411 → "Ready for review" → merge. **Order is
load-bearing** (host first, merge second — the PR explains why). A backup
already exists at `/home/guardian/backups/pre-dbmove/cortex_20260802_035217.db`.

## 2. Anthropic — one message that matters, one optional (~5 min)

Correction to the earlier framing: **(b) is the real item; (a) is optional
insurance.** The Claude API exists to power products, and Cortex uses
operator-funded API keys — §D.4 restricts reselling model *access*, which
Cortex does not do. Nothing is blocked on (a). Its only lasting value is
having a written answer on file when an enterprise security review asks
whether you have provider authorization. Send it if you want the artifact;
skip it without cost.

Send via your Anthropic Console support/sales contact. Drafts; edit freely.

**a) Commercial terms comfort check — optional:**

> Subject: Confirming §A.1 posture for a product built on the Claude API
>
> Hi — I operate Cortex, a coding-agent product built on the Claude API
> under operator-funded API keys. Customers buy prepaid credits denominated
> in completed, verified engineering tasks — never in tokens or model
> access; token costs are internal COGS and are not resold or quoted to
> users. My reading is that this sits squarely within §A.1 ("power products
> and services…") rather than §D.4's resale restriction, and the product
> was designed specifically to stay on the right side of that line. Could
> you confirm that understanding, or point me to anything you'd want
> structured differently? Happy to share schema-level detail.

**b) Rate limits — send this one:**

> Subject: Spend-cap / tier increase request
>
> Same product as above. We're pre-launch; agentic coding workloads mean
> spiky, bursty usage at launch (a good launch day resembles a usage
> anomaly). I'd like to start the tier-increase conversation early rather
> than hit caps with live customers: what's the path to a higher monthly
> spend cap and acceleration-limit headroom on our org?

## 3. UNVERIFIED pricing row — a yes/no (~1 min)

VERIFIER.md sells tasks with no derivable checks as labeled **UNVERIFIED**:
normal credit price, no verified badge, no refund promise.
**Recommendation: approve.** It preserves revenue on legitimate work
(docs, exploration), keeps "verified" meaning something, and the refund
economics stay clean. The alternative — refusing such tasks — donates that
work to competitors. Reply "approved" (or object) and the row unblocks.

## 4. SOC 2 — the clock, demystified (~15 min when triggered)

**What it is not:** it has nothing to do with ownership. Cortex is yours,
fully, certified or not. SOC 2 is a security report card *you commission
about your own company*. Big customers' procurement rules — their internal
policy, not law — forbid buying from vendors without one. That is the only
thing it unlocks.

**What it actually is:** an auditor attests your controls (access, change
management, incident response) over a window. Type I = point in time
(~2–3 months away once started); Type II = observed over 3–6 months —
that observation window is the "clock" that can't be compressed, which is
the entire reason to start before the first enterprise deal, not during it.

**What only you can do:** create the account, connect billing, sign the
auditor engagement. ~1 hour total, spread out.

**What agents then do (the other ~90%):** policy drafting, evidence
wiring (GitHub/cloud integrations), control remediation across the repo —
all of it agent-shaped work once the platform account exists.

**Concrete path:** a compliance-automation platform (Vanta is the default
recommendation for a company this size; Drata is the credible alternative —
have the agent doing setup compare current pricing), then their auditor
marketplace for the engagement. Ballpark: $10–20k/yr platform +
$7–15k audit.

**Trigger, so this isn't premature:** start when the first Team-tier
customer pays or the first enterprise inquiry lands — whichever comes
first. Until then it's a line item, not a task.

## 5. Entity, terms, Stripe — a launch gate, not a someday item (~1 hour)

**Not parked.** "Ready for external users" includes "an external user can
pay you," and none of this can be done at the last minute: entity filing takes
days, Stripe onboarding can take days more and sometimes asks for documents,
and terms want a lawyer's pass. Start it in parallel with the build rather
than discovering it on launch week. Agents draft everything; the filing,
the signature, and the account creation are yours.

- **Legal entity.** Stripe onboards a business identity, not a person, and
  customers need someone to contract with. Filing is yours; an agent can
  compare structures and pre-fill.
- **Terms of Service + privacy policy.** Customers accept *your* terms;
  they must cover the credit's non-refundability rules (CREDITS.md's refund
  policy is the source of truth — the ToS must not contradict it), data
  handling for the repo index (CONTEXT.md's embeddings caveat), and
  acceptable use. Agents draft; a lawyer's one-pass review before launch is
  cheap relative to the risk.
- **Stripe account + tax settings.** Yours to create; the integration is
  agent work once it exists.

## 6. The dogfood loop — Josh's standing job once deploys unfreeze

Not a one-off. Each time a lane lands something a user would touch, Josh
uses it as a customer, and the agent that shipped it asks for that pass
before calling the task done. Concretely, in the order the build reaches
them:

| After | Josh does | Watching for |
|---|---|---|
| #411 + a fresh deploy | log in to the live app at all | does the deployed binary actually serve the current build |
| F1 (surface amputation) | click every pane of mission control | anything that is still BYOK-era, or a screen with no purpose |
| V1–V5 (verifier) | run a real task on a real repo end to end | is the receipt legible; would you believe it |
| #437 live + F2 (ledger) | spend credits, watch the balance | does the number shown match what you think you spent |
| Cost-confidence work (SURFACE) | start a run and read the price first | is the quote clear enough that you'd press go without anxiety |
| Onboarding (F4) | sign up as a stranger, from zero | the 10-minute install-to-verified-PR target, measured on a human |

The last row is the one that decides launch readiness, and it cannot be
faked by an agent: an agent already knows how the product works.

## 7. Five decisions from the research pass (~10 min; added 2026-08-06)

[RESEARCH-2026-08.md](RESEARCH-2026-08.md) surfaced five product/positioning
calls. Each is yours because it is customer-facing; each has a recommendation
so a yes is one word. Engineering recommendations from the same brief went
into VERIFIER.md/CONTEXT.md amendments and need nothing from you.

1. **Position against the H1-2026 repricing wave, by name.** Copilot, Codex,
   and Cursor all converted to effort metering within four months; bill-shock
   is the category's dominant press right now. Proposed artifact: a "your
   invoice, reconciled" page — every line item links a charge to a replayable
   receipt. *Recommendation: approve; timing window is live.*
2. **The Plan Receipt.** Sell verified decomposition: the plan DAG posted for
   approval with per-leaf derived checks, VERIFIED/UNVERIFIED label, credit
   price, and lease claim; archived with run evidence; re-planning always
   free. Nobody sells this; Kiro's tasks.md is closest and has none of the
   four. *Recommendation: approve as a SURFACE.md F3/F4-era feature, new
   PLAN-RECEIPT.md decision record when picked up.*
3. **Public, shareable receipt URLs.** A receipt page a developer links in PR
   discussion or a CFO cites in audit — the viral unit of the trust story.
   Read-only view of V6's renderer. *Recommendation: approve, post-V7 only
   (a public receipt on an unhardened verifier cuts the other way).*
4. **Kiro interop.** Accept EARS-format acceptance criteria as contract-check
   input — teams already writing Kiro-style specs hand them to Cortex
   unchanged: their spec, our sandbox, our receipt. *Recommendation: approve
   in principle; schedule after F3.*
5. **Agent HQ posture.** GitHub's multi-vendor dashboard commoditizes "watch
   your agents run"; cortex.heyvera.org must lead with receipts/ledger/leases
   (which Agent HQ cannot render). Separately: evaluate listing Cortex as an
   Agent HQ agent for distribution. *Recommendation: adopt the differentiation
   framing now; defer the listing evaluation until F3 exists.*

---

*Standing division of labor this file encodes: agents research, draft,
build, and verify; Josh uses the product and holds the credentials. If an
item on a future "blocked on Josh" list is neither of those, it wasn't
compressed hard enough.*
