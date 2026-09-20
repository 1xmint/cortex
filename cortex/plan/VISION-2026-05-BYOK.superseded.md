# Cortex — Product Vision & Master Plan

> This is the single source of truth for what Cortex is, what's built, and what we're building.
> If another doc contradicts this one, this one wins.
> Last updated: 2026-05-29

---

## What Cortex Is

Cortex is a **development environment** users own that includes a mobile-first AI coding partner. Every user gets their own hosted container with pre-installed AI tools (Claude CLI, OpenAI CLI) and Cortex orchestration software. It's not a chatbot service — it's your personal AI-enabled development workspace.

**Primary audience:** Vibe coders. People with a $20/mo Claude or ChatGPT subscription who want to build things faster. They don't want to manage infrastructure or think about tokens.

**Secondary audience:** Teams and dev orgs. Multiple people sharing repos, coordinating work, sharing subscription access without sharing passwords.

**Business Model:** $6.99/mo unlimited BYOK platform. Users bring their own API keys, Cortex provides orchestration, mobile-first interface, and coordination. Optional container integration via Replit API where users pay Replit directly for compute when needed — Cortex handles provisioning but stays out of compute billing.

**Cortex stands alone.** No external protocol dependencies. Identity is Clerk. Billing is Stripe. Orchestration is Cortex's own engine. If/when Soma protocol matures, Cortex becomes the first agent to use it — but Cortex never requires it.

---

## The Three Surfaces

Every user interacts through three connected surfaces. They're views of one engine, not separate products.

### 1. Project Chat (Private — per user, per project)

Your private workspace with Cortex for a specific project/repo. This is where you vibe and code. Nobody else sees your conversation.

**What you do:**
- Talk about your codebase — fix bugs, add features, refactor, review
- Brainstorm architecture, challenge ideas, think out loud
- Vibe — "I'm stuck, what do you think?"
- Cortex challenges back — "that'll work but have you considered X?"

**Under the hood:**
- Intent classification routes to the right model tier (cheap vibing, expensive coding)
- When real work is requested, creates a Task visible to your Task Manager
- Code work runs in isolated git branches — never touches main directly
- Cortex remembers your style across sessions

**Rules:**
- Chat is NOT the source of truth for task status — Task Manager is
- Chat attaches to a project, not scattered across retries
- Private means private — teammates see your tasks, never your chat

### 2. Personal Task Manager (Your command center)

Your hub across ALL projects and teams. You talk to it like a capable assistant who sees everything you're involved in.

**What you do:**
- "Start a new project" → creates project, links repo
- "How's my team's sprint going?" → aggregates status across that team
- "Did Alice finish the auth fix?" → checks task status (not her private chat)
- "Share my Claude subscription with Team B for 1 week or 1M tokens" → creates scoped delegation
- "What did I ship this week?" → activity summary
- "Create a new team called Backend" → team setup

**What you see:**
- Inbox: tasks needing your attention across all projects/teams
- Projects: all your projects with status
- Teams: all teams you belong to
- Live Map: your active work being orchestrated (text-based first, visual later)

**Rules:**
- Can READ team status, but team writes go through Team Task Manager
- Credential delegations managed here (you share YOUR subscriptions)
- Queries like "how's my team?" are database lookups, not LLM calls — free

### 3. Team Task Manager (Shared orchestration)

The shared surface for a team. Every member can see it. This is where private work becomes visible and coordination happens.

**What everyone sees:**
- All tasks across the team's projects
- Who's working on what
- Status of every run (planned, running, blocked, done)
- Dependencies and blockers
- Live Map: all members' work in real time

**Roles:**
- **Lead:** Full control — add tasks, prioritize, assign, approve, cancel, manage delegations
- **Member:** Work on tasks, suggest new ones (lead approves), view everything
- **Viewer:** Read-only

**How conflicts are prevented:**
- Alice's Cortex starts work on auth → creates Task in Team Task Manager
- Task Manager acquires a lock on affected files
- Bob's Cortex tries auth too → Task Manager says "Alice is already on this, here's her task"
- Code runs in isolated git branches, merged only after review
- No two people can hold a lock on the same resource

**How private connects to shared:**
- Private chats emit task status (created, progress, done, blocked)
- The conversation stays private — only the WORK OUTPUT is shared
- Like git: your local branches are private, pushed commits are shared

---

## How The Surfaces Connect

```
User
 ├── Personal Task Manager (one per user)
 │    ├── sees all teams and projects
 │    ├── manages subscription delegations
 │    └── personal Live Map
 │
 ├── Team Memberships
 │    ├── Team A (role: lead)
 │    │    ├── Team Task Manager (shared)
 │    │    └── Projects
 │    │         ├── Project X → your private chat
 │    │         └── Project Y → your private chat
 │    └── Team B (role: member)
 │         ├── Team Task Manager (shared, limited write)
 │         └── Projects
 │              └── Project Z → your private chat
 │
 └── Personal Projects (no team)
      └── Side Project → your private chat
```

Communication:
```
You ←chat→ Your Cortex (private, per project)
                ↓ emits tasks
         Team Task Manager (shared)
                ↓ queried by
         Your Personal Task Manager
                ↓ also queried by
         Teammate's Personal Task Manager
```

---

## Chat Economics

Core promise: **cheap endless vibing, expensive thinking only when needed.**

| Interaction | Route | Cost | Example |
|---|---|---|---|
| Vibing, chatting, brainstorming | Fast model (Haiku / GPT-4.1-mini) | Near-zero | "What do you think about this?" |
| Task Manager queries | Database lookup + template | Free | "How's my team doing?" |
| Architecture, review, analysis | Balanced model (Sonnet / GPT-4.1) | Moderate | "Review this for security" |
| Code execution, complex work | Powerful model (Opus / GPT-5.5) | Expensive | "Implement the auth refactor" |

Intent classifier determines tier. User can override via session controls. Model transitions are seamless — user just notices Cortex thinking harder sometimes.

---

## Cortex Personality

NOT a generic AI assistant. The best work partner a person could ask for.

- **Vibes** — doesn't just execute, actually engages
- **Cares** — "you seem stuck, want to step back?"
- **Challenges** — "that'll work but X is simpler"
- **Learns** — remembers your style, preferences, domain
- **Opinionated** — has recommendations, doesn't just list options

Personality is consistent across model tiers (system prompt carries it). Personality is per-user, not per-project — YOUR Cortex everywhere.

---

## Current State (What's Actually Built)

### Backend (Rust — `crates/api/`)

| System | Status | Notes |
|--------|--------|-------|
| **Database** | WORKING | 15K line SQLite layer, 100+ methods, all migrations |
| **Clerk Auth** | WORKING | JWT verification, JWKS caching, admin bypass |
| **Stripe Billing** | WORKING | Checkout, portal, usage gates, promo codes |
| **Docker/BYOS** | PARTIALLY WORKING | Container create/start/stop works. Auth flow (CLI OAuth) broken — container exec doesn't reliably return OAuth URL |
| **Chat + SSE** | WORKING | Streaming responses, intent classification, tier routing |
| **LLM Client** | WORKING | Claude CLI + OpenAI Codex CLI spawning, API key mode |
| **Scheduler** | WORKING | Step orchestration, leasing, worker registration |
| **WebSocket** | WORKING | Worker connections, step streaming, lease management |
| **Rate Limiting** | WORKING | Per-user, per-endpoint |
| **Metrics** | WORKING | Prometheus, subsystem health |
| **GitHub** | PARTIALLY WORKING | Repo listing + import endpoints exist, untested on prod |
| **Admin** | WORKING | Workers, containers, promo codes, audit log |
| **Social** | WORKING | Full social network (profiles, posts, follows) — this is HeyVera Social, not Cortex core |
| **Conversations** | WORKING | Basic CRUD, message storage |
| **Credentials** | WORKING | Multi-credential, encryption, CRUD |

### Frontend (React — `cortex/src/`)

| Component | Status | Notes |
|-----------|--------|-------|
| **App Shell** | WORKING | Routing, auth gate, sidebar, header, modals |
| **Chat** | WORKING | SSE streaming, messages, timeline, composer |
| **Onboarding** | BUILT (needs rework) | 4-step wizard — should become tutorial + mock demo |
| **Settings** | PARTIALLY WORKING | Provider auth UI works, budget/notifications scaffolded |
| **Billing** | WORKING | Pricing cards, trial banner, Stripe checkout |
| **Task Manager** | WORKING | Kanban board, drag-drop, evidence, approvals |
| **Operations Room** | WORKING | Summary, graph, approval queue |
| **Personal Task Manager** | WORKING | Cross-group overview |
| **Admin** | WORKING | Stats, promo codes, workers |
| **Projects** | PLACEHOLDER | Shell exists, backend incomplete |

### What's Broken Right Now

1. **Container-as-Service model** — current architecture treats Cortex as a service that manages user containers, which violates AI provider ToS. Need to pivot to Container-as-Product model where users own their development environments.
2. **Auth flow architecture** — current OAuth flow has Cortex routing subscription credentials, which is prohibited. Need frictionless auth where Cortex helps users authenticate their tools inside their own containers.
3. **Frontend tests on cortex.heyvera.org only** — Replit webview is unreliable, never use it.
4. **Chat personality** — generic "AI assistant" system prompts. Needs the Cortex personality.
5. **Project system** — frontend shell exists but no real project lifecycle in backend.

### What Works But Isn't Cortex Core

- Social features (profiles, posts, follows) — HeyVera Social, separate binary (`heyvera-server`)
- Soma identity endpoints — real code, but Cortex doesn't depend on it
- Vera observation layer — real code, but not needed for Cortex v1

### Backend Architecture

Two separate binaries from the same workspace:
```
./target/release/cortex-server    ← cortex.heyvera.org (port 3001)
./target/release/heyvera-server   ← heyvera.org (port 3002)
```

- `crates/cortex-server/` — Cortex binary (chat, docker, runs, scheduler, GitHub)
- `crates/heyvera-server/` — HeyVera Social binary (posts, follows, profiles, messaging)
- `crates/api/` — shared library (db, auth, billing, crypto, metrics)
- `crates/shared/` — future home of truly shared infra (migration target)

Each binary mounts only its own routes. They can be deployed, scaled, and updated independently.

---

## Data Model (What Cortex Needs)

### Already Exists
```
users, conversations, messages — basic chat
runs, steps, step_dependencies, step_attempts — task orchestration
user_credentials, credential_data — encrypted credential storage
workers, worker_sessions — agent registration
decisions, outcomes — routing history
usage_events — token/cost tracking
```

### Needs to Be Added
```sql
-- Teams
teams (id, name, owner_id, created_at)
team_members (team_id, user_id, role, joined_at)

-- Projects (link repos to teams or personal)
projects (id, team_id NULL, name, repo_url, created_at)

-- Link conversations to projects
conversations.project_id → projects.id

-- Link runs to projects and source chats
runs.project_id → projects.id
runs.source_conversation_id → conversations.id

-- Subscription delegation (Cortex-native, no Soma dependency)
subscription_delegations (
    id, credential_id, delegated_to_team_id,
    max_tokens, tokens_used,
    expires_at, status, created_at
)

-- User memory (what Cortex learns about you)
user_memory (
    id, user_id, category,  -- style, preference, domain, correction
    content, created_at, updated_at
)

-- Resource locks (conflict prevention)
resource_locks (
    id, resource_type, resource_key,
    holder_user_id, holder_run_id,
    acquired_at, expires_at,
    UNIQUE(resource_type, resource_key)
)
```

---

## Build Sequence

### Phase 0: $6.99 BYOK Platform + NPX Tool (NOW)

**Goal:** Ship dual approach - standalone NPX tool for subscriptions + hosted BYOK platform.

**NPX Tool (Free):**
- [ ] Extract dual-brain system from archives
- [ ] Rebuild as `npx cortex` without data-tools dependency  
- [ ] Persistent conversation + dual orchestration (Claude + GPT)
- [ ] Works in any shell (Replit, local, cloud containers)
- [ ] Zero ToS issues (user runs tool themselves)

**Hosted Platform ($6.99/mo):**
- [ ] BYOK API key orchestration 
- [ ] Mobile-first interface
- [ ] Optional Replit container integration (user pays Replit directly)
- [ ] File storage + coordination covered by $6.99
- [ ] Deploy to cortex.heyvera.org

### Phase 1: Project Chat Perfect (Week 1-2)

**Goal:** One user, one project, private chat that feels amazing.

- [ ] Chat routes correctly to BYOS container or BYOK API
- [ ] SSE streaming smooth, no buffering
- [ ] Intent classification: `/fix`, `/explore`, `/review`, `/think` + automatic detection
- [ ] Tier routing: vibing = cheap, thinking = moderate, coding = powerful
- [ ] Conversation CRUD: create, list, load, delete, auto-title
- [ ] Cortex personality in system prompts — vibes, challenges, cares
- [ ] Personality consistent across model tiers
- [ ] Glass morphism, animations, premium dark theme
- [ ] Markdown + syntax-highlighted code blocks + copy
- [ ] Slash commands with autocomplete
- [ ] Loading states, error states, empty states all polished
- [ ] Mobile responsive

### Phase 2: Personal Task Manager (Week 2-3)

**Goal:** User can create projects, see tasks, manage their work from one place.

- [ ] `projects` table + CRUD endpoints
- [ ] `conversations.project_id` column
- [ ] Create project from chat ("start a new project")
- [ ] GitHub repo import → project created
- [ ] Project list in sidebar with status
- [ ] Personal Task Manager chat: talk about all your work
- [ ] "How are my projects?" → DB query, formatted response (free)
- [ ] "What did I ship this week?" → activity summary
- [ ] Tasks created from Project Chat appear in Task Manager
- [ ] Task status lifecycle: inbox → ready → active → blocked → review → done

### Phase 3: Billing & First Impression (Week 3-4)

**Goal:** People can pay, and the first experience converts them.

- [ ] Stripe checkout: $6.99/mo or $69/yr
- [ ] Subscription gates product access
- [ ] Mock data demo: prospects see fake project, fake chat, fake tasks — spectacular
- [ ] Tutorial for paying users: link subscription → import repo → first message → coding in 60 seconds
- [ ] Tutorial button always accessible
- [ ] Promo codes + referral links
- [ ] Usage tracking visible but not anxiety-inducing

### Phase 4: Teams (Week 4-6)

**Goal:** Multiple people collaborate on shared repos without conflicts.

- [ ] `teams` + `team_members` tables
- [ ] Create team, invite members
- [ ] Projects belong to teams
- [ ] Team Task Manager: shared surface, shows all members' tasks
- [ ] Private chat → task emission to team
- [ ] Resource locks: Task Manager prevents overlapping work
- [ ] Role-based permissions: lead / member / viewer
- [ ] "How's my team doing?" via Personal Task Manager
- [ ] Cross-team task creation ("add this bug to all teams")

### Phase 5: Subscription Delegation (Week 6-7)

**Goal:** Share subscription access with teams, scoped and bounded.

- [ ] `subscription_delegations` table
- [ ] "Share my Claude sub with Team B for 1 week or 1M tokens"
- [ ] Token counting per delegation
- [ ] Auto-expiry (time or tokens, whichever first)
- [ ] Manual revoke with graceful in-flight handling
- [ ] Delegation visible in Personal and Team Task Manager
- [ ] Team members use delegated credentials in their containers

### Phase 6: Agent Memory (Week 7-8)

**Goal:** Cortex learns your style and gets better over time.

- [ ] `user_memory` table
- [ ] Cortex learns coding style, preferences, domain from chat patterns
- [ ] User can tell Cortex things: "I prefer functional style"
- [ ] Memory view: see what Cortex knows about you
- [ ] Edit/delete memories
- [ ] Memory injected as context in every message
- [ ] Consistent across projects (it's YOUR Cortex)

### Phase 7: Visual Live Maps (Week 8+)

**Goal:** See your work and your team's work orchestrated visually.

- [ ] Network graph: spatial topology of projects, tasks, agents
- [ ] Timeline: left-to-right flow of work
- [ ] Board: kanban status columns
- [ ] Real-time updates via SSE
- [ ] Personal and team scoped views
- [ ] Click-to-inspect: task detail, logs, artifacts

### Phase 8: Advanced Orchestration (Week 10+)

**Goal:** Cortex autonomously decomposes, executes, tests, and heals work.

**Hierarchical AI Org Chart:**
- [ ] Bidirectional model hierarchy: Search (workers) ↔ Execute (ICs) ↔ Think (managers)
- [ ] DELEGATE DOWN: Execute → Search for mechanical work (grep, tests, renames)
- [ ] ESCALATE UP: Execute → Think when stuck/low confidence via self-assessment tokens
- [ ] BOUNCE DOWN: Think → Execute with critique and work orders
- [ ] Asymmetric context envelopes: Work Orders (200 tokens) down, Escalation Packets up
- [ ] Bandit-learned escalation thresholds: system learns when to skip doomed attempts
- [ ] Only successful, reconciled work reaches user after internal bounces/fixes

**Traditional Orchestration:**
- [ ] DAG decomposition: "build auth" → search → design → implement → test → review  
- [ ] Git branch-per-task isolation
- [ ] Dynamic heal: test fails → auto-fix → re-test
- [ ] Evidence-backed completion: done = diff + tests + verifier
- [ ] Multi-worker scheduling with fairness
- [ ] Lock enforcement: no two workers on same files

---

## What Cortex Does NOT Depend On

| Thing | Relationship | Why |
|-------|-------------|-----|
| **Soma Protocol** | Future integration, not dependency | Soma isn't finished. Cortex uses Clerk for identity, its own DB for trust/evidence |
| **Vera Network** | Future integration, not dependency | Vera is 0% done. Cortex tracks its own interactions |
| **ClawNet** | Separate product | ClawNet is API infrastructure. Cortex is a coding platform. Different products. |
| **DualBrain** | Foundation | Archived dual-brain system (v4.6.0) provides foundation for NPX tool - sophisticated orchestration with Claude + GPT |
| **Social features** | HeyVera Social, not Cortex | Posts, follows, communities are a different product sharing the same backend |

When Soma matures, Cortex becomes the first agent to use it — Soma Hearts for agent identity, delegation for credential sharing, receipts for proof-of-work. But that's Phase 10+, not now.

---

## What Success Looks Like

**Free NPX User:**
1. Opens any shell → `npx cortex` → persistent AI chat with dual orchestration
2. Uses their own Claude + GPT subscriptions seamlessly
3. Conversation persists across sessions, works anywhere
4. Zero setup, zero hosting costs, zero ToS issues

**$6.99 Platform User:**
1. Lands on cortex.heyvera.org → signs up → pastes API keys
2. Mobile-first AI coding with smart orchestration
3. When needs containers → seamless Replit integration (user pays Replit directly)
4. Teams coordinate through shared projects, individual billing

**Developer/Team Experience:**
1. Serious vibe coders get sophisticated AI orchestration with their subscriptions
2. Teams get coordination without subscription sharing complexity  
3. Transparent costs, no markup on AI usage
4. Mobile-first experience unavailable elsewhere

**The moat:** Only tool that provides both free subscription orchestration (NPX) AND premium BYOK platform, with mobile-first UX and team coordination that doesn't require users to trust third parties with their AI spend.
