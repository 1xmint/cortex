# Working in this repository

Read this first, then [STATE.md](STATE.md). They outrank the code and the
wording of any single request.

## What this is for

Cortex is the brain of a creation system, in the way Jarvis is to Iron Man: a
superintelligent assistant to a human creator, working alone or in a team. A
person describes what they want in plain words. Cortex understands it,
designs it, builds it with the best tools and services available, proves it
works, and remembers what it learned so the next result is better.

Software is the first thing Cortex proves itself on. The same brain must later
drive work that mixes code, art, sound, video and animation. A one-prompt
complete game is the kind of result it is built for.

Who it serves: anyone who wants professional work done, from developers who
want to learn every decision to people who never want to see a repository.
Each person sets how involved they are in their own *working agreement*.

## Non-goals

- Cortex is not a model gateway or a token reseller. The models are tools;
  Cortex is the one using them.
- It is not a fixed pipeline per medium. A creation is connected pieces that
  share one project bible, not separate departments.
- It does not claim "done" on a model's word.

## Invariants (cite by number)

1. **Independent proof is the only truth.** A result is done when checks
   Cortex runs itself say so. A tool's or model's own report is a note, never
   a gate.
2. **Every check must be able to fail.** A test or gate that cannot go red is
   removed or fixed, not kept.
3. **Untrusted work runs isolated.** Model-authored code never gets host
   credentials, provider keys, or the user's wider machine. If isolation
   cannot be set up, the step is blocked; running unisolated is never the
   fallback.
4. **The person's working agreement is honoured.** Cortex shows, asks and
   notifies exactly as the person has agreed, and never acts outside it.
5. **The gold rule.** Every technology choice is made for the best long-term
   result, not the fastest build. Each choice gets an ADR that scores the
   options on: correctness and reliability over hours-long runs, security,
   long-term maintainability, ecosystem fit for that job, performance where it
   matters, and the cost of another language boundary. Build speed only breaks
   ties. A mix of languages is fine when each is truly best for its part.

## One source of truth for each kind of fact

| Question | Its only home |
|---|---|
| What does it do? | Code and tests |
| How good is it? | Scoreboard results, produced by real runs, never hand-written |
| Why is it this way? | [docs/adr/](docs/adr/README.md). Accepted ADRs are never edited; a new ADR supersedes an old one |
| What is true now, and what is next? | [STATE.md](STATE.md) |
| What is it for, and what are the rules? | This file |
| How is one piece built? | A spec and a plan in `docs/superpowers/`, marked landed when merged |

Do not add other summary, status or handoff files. If a fact has no home in
this table, it goes in STATE.md or nowhere.

## How work lands

- Every change goes through a pull request. `main` is protected; the required
  checks must pass, and a PR may be merged once they are green.
- A PR that changes anything outside `docs/` and Markdown files must update
  STATE.md, or say why not in its description with a line starting
  `STATE-unchanged:`.
- Tests run on GitHub, not on the owner's PC. Locally, run only the compile
  and lint checks for what you changed.
- Claims need evidence: name the command you ran and what it printed. If
  something is untested, say so in the PR.

## Checks that keep the docs honest

`.github/workflows/docs-truth.yml` fails the build when:
- a link or file path in any Markdown file does not exist;
- `docs/adr/README.md` is not exactly what `scripts/adr-index.sh` generates;
- STATE.md is over its line cap, or a code change arrives without a STATE.md
  update or a `STATE-unchanged:` reason.

## Talking to the owner

Josh describes the product in plain language and does not read code. Lead with
the answer, use small words, exact paths, and give one recommendation with
every decision that is his to make: what the product does, money,
credentials, who can see data, and anything public or irreversible.
