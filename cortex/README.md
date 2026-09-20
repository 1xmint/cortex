# Cortex web client

The browser client for Cortex: the mission view, the cost and ledger surfaces,
the project and chat panels, and the settings and onboarding flows. React +
TypeScript, built with Vite, served by Cloudflare Pages.

It talks to `crates/api` over HTTP. `src/lib/cortexApi.ts` is the only module
that knows the wire format; everything else goes through it.

## Running it

```sh
npm install
npm run dev
```

`npm run build` type-checks (`tsc -b`) and then bundles. `npm run lint` runs
eslint. `npm run seed` populates a local state fixture for working on views
without a live API.

## CI

The `cortex` job builds this directory and is a required check, so a type error
here blocks the merge. `npm-audit (cortex)` is also required. `lint` is
advisory today.

There are no frontend tests yet. Adding the first one, and a floor that can
only go up, is tracked in the harness plan (Phase 16-21).
