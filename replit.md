# Cortex

## Overview
**Cortex** is an AI coding/agent platform, powered by a Rust backend and a React frontend.

- **Frontend**: React 19 + Vite + Tailwind CSS, running on port 5001
- **Backend**: Rust (Axum), compiled binary at `target/release/cortex-server`, running on port 3001
- **Database**: SQLite at `.cortex/cortex.db`

## Running the project
Two workflows run in parallel (managed via `.replit`):
1. **Backend API** — runs `target/release/cortex-server` on port 3001
2. **Start application** — runs `cd cortex && npm run dev` (Vite dev server on port 5001, proxies `/api` to port 3001)

## Building the Rust backend
After any Rust source changes, rebuild the binary:
```bash
cargo build --release -p cortex-server-bin
```

## Auth
- **Clerk** is the auth provider. It's optional: if `VITE_CLERK_PUBLISHABLE_KEY` / `CLERK_SECRET_KEY` are absent, the app runs in dev mode (all requests treated as user "local").
- Set secrets via Replit's secret store (not `.env` files).

## Key environment variables
| Variable | Purpose |
|---|---|
| `CLERK_SECRET_KEY` | Clerk backend secret (enables JWT verification) |
| `VITE_CLERK_PUBLISHABLE_KEY` | Clerk frontend publishable key |
| `CORTEX_ADMIN_EMAILS` | Comma-separated admin emails |
| `STRIPE_SECRET_KEY` | Stripe billing (optional) |
| `ANTHROPIC_API_KEY` | Anthropic API for AI features |
| `OPENAI_API_KEY` | OpenAI API for AI features |
| `CORTEX_PORT` | Backend port (default: 3001) |

## User preferences
- Admin email: jfair1028@gmail.com
