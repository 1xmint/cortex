# PR brief — make product route ownership executable

## Scope

- Check in a literal method/path manifest for all 205 current route templates.
- Enforce the approved split: 92 Cortex-only, 98 Socials-only, and 15 independently duplicated.
- Remove all Socials, Pulse, Clerk-account-webhook, and Socials account-admin paths from the Cortex router.
- Remove `/api/auth/status` and every Cortex-only path from the HeyVera router.
- Delete the combined legacy router and move its tests to the owning product router.
- Return `404` for unmatched API-shaped paths instead of serving an SPA fallback.
- Replace HeyVera's broad Caddy API forwarding with explicit product matchers and deny fallbacks.

## Acceptance evidence

- `route-manifest.csv` contains exactly 205 unique templates and their methods.
- Source-derived router maps exactly equal their manifest projections.
- Every Socials-only method/path returns `404` from Cortex.
- Every Cortex-only method/path returns `404` from HeyVera.
- `/api/auth/status` is Cortex-only.
- Duplicated billing usage resolves to independent Cortex and Socials behavior.
- Existing Cortex, Socials, Soma-isolation, and end-to-end tests use an explicit product router.

## Operational boundary

This branch changes only repository code and inactive proxy configuration. It does not reload Caddy, deploy either service, change DNS, push a branch, or mutate production state.
