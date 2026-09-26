# Phase 27.1 progress

Done: verdict_class declared at plan time (crates/core/src/diff_surface.rs
declare_verdict_class + 6 unit tests), threaded through scheduler.rs
freeze_step_quote and pricing.rs credits_for_verdict_class (+3 unit tests,
div_ceil replaced with (x+1)/2 since i64::div_ceil is unstable on this
toolchain), Receipt struct/ledger.rs wired with verdict_class+charged_credits,
frontend Receipt.tsx badge/copy + Receipt.test.tsx (3 tests) + ci.yml floor
26->29. Frontend npm test (165 passed) and npm run build both pass. Pushed as
WIP commit bd4c0461 to cortex/feat/verdict-class-at-plan.

Next: cargo build -p cortex-api was failing with "declare_verdict_class not
found in cortex_core::diff_surface" despite cortex-core building clean alone
and the function existing (line 148) -- rebuild in progress in background to
confirm if it was a stale-cache fluke; if it recurs, check for a duplicate
cortex-core path dependency / workspace member shadowing. Still need:
cargo fmt/clippy/test --workspace, the exam_integrity end-to-end check
(verification_driver.rs believed already correct, not independently tested),
API/billing integration test for authored=ceil(full/2) charge+refund, the
testing/stub-provider/ e2e scenario (not started), and EngineReceipt.tsx /
ReceiptsPane.tsx investigation (not started).

Watch out for: use x86_64-pc-windows-msvc toolchain, not the default gnu one
(gnu's mingw linker is broken here, `cargo +stable-x86_64-pc-windows-msvc`).
CRLF fmt warnings across ~70 unrelated files are pre-existing, not from this
change -- don't fmt-fix files this task didn't touch. No new migration is
needed (verdict_class rides in step_work_contracts.contract_json). Budget is
very low (near turn limit) -- likely to hand back as PARTIAL with a draft PR
covering only the tested subset (classification + pricing + receipt display)
and the remaining items listed explicitly.
