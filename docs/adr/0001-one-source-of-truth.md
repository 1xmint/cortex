# 0001. One source of truth for each kind of fact

Status: Accepted
Date: 2026-10-18

## Context

The previous Cortex kept vision, plan, state and architecture in many
overlapping documents, including a plan over eight thousand lines long. They
drifted from the code and from each other; its architecture document opened
with an amendment saying parts below it were no longer true. Agents and
people could not tell which document to trust.

## Options

1. One large living document. Easy to find, but it goes stale first in its
   status sections and then cannot be trusted anywhere.
2. Many documents with conventions only. This is what drifted before.
3. A small fixed set of homes, one per kind of fact, enforced by CI.

## Decision

Option 3. The homes are listed in AGENTS.md: code and tests for behaviour,
scoreboard results for quality, ADRs for reasons, STATE.md for now and next,
AGENTS.md for purpose and rules, and per-slice specs and plans for how a piece
is built. Accepted ADRs are never edited; a new ADR supersedes an old one and
the old one's status line points to it.

CI enforces it: every link and relative path in Markdown must resolve; the ADR
index is generated and must match; STATE.md has a line cap and must be
updated (or a reason given) by any PR that changes more than docs.

## Consequences

Docs stay small and checkable. The cost is a little friction on every PR, and
the checks cannot catch a sentence that is wrong but well-linked; that is left
to review. Revisit if the checks block more good PRs than they catch drift.
