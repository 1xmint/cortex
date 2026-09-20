//! Typing the diff: which part of a delivery is the work, and which part is the
//! exam that grades it.
//!
//! Phase 27.2. The V3 design froze the check *argv* at dispatch so a task could
//! not rewrite its own exam — `verification_driver.rs` says so at the point it
//! loads the frozen specs. What it did not freeze is what that argv **reads**.
//! `cargo test --all` frozen at dispatch still executes against the tree the
//! agent delivered, and that tree contains the agent's edits to `tests/`. The
//! detached checkout is a clean checkout *of the delivered commit* — the very
//! commit whose test files the agent controls. Cleanliness was never the
//! exposure.
//!
//! # Why this is not a prohibition
//!
//! Modifying the exam is legitimate. Test-driven work does it by definition,
//! and Phase 24.2's characterization testing — the on-ramp that turns an
//! unverifiable brownfield repository into a customer — *is* exam authorship.
//! Forbidding it would break the product.
//!
//! The rule is that a verdict produced under exam authorship is **a different
//! kind of claim**, and may never be rendered as though it were the same one.
//! Hence [`VerdictClass`], declared at plan time and carried onward.
//!
//! # What this module does not know
//!
//! Classification here is by path. That is deterministic and reproducible,
//! which is what a receipt needs, and it is **incomplete for Rust**: a unit test
//! in this codebase lives inside the file it tests, as `#[cfg(test)] mod tests`
//! at the bottom of `src/whatever.rs`. No path glob can see it. An agent that
//! weakens an assertion inside such a module edits a `subject` path by this
//! module's reckoning, and a `strong` verdict over that diff is weaker than it
//! sounds.
//!
//! That gap is real and is stated rather than papered over. Closing it needs
//! the build graph — Phase 25.2 already reads it — to say which targets are
//! test targets, plus the AST comparison Phase 27.2's signal table describes.
//! Both are follow-ups. This module is the surface they will attach to.

use serde::{Deserialize, Serialize};

use crate::check_derivation::EcosystemFacts;

/// Which of the three surfaces a changed path belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffSurface {
    /// Production code the contract asked to change. The thing being graded.
    Subject,
    /// Anything a frozen check reads as ground truth. Changing it changes the
    /// grade.
    Exam,
    /// Neither graded nor grading: lockfiles, generated output, documentation.
    Incidental,
}

/// What kind of claim a verdict over this delivery can be.
///
/// Declared at plan time, on the Plan Receipt, *before* approval — so that
/// delivery cannot choose it after the fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum VerdictClass {
    /// The exam surface is byte-identical between base and delivered tree. The
    /// battery that graded the work is the battery the customer had before
    /// Cortex touched anything.
    Strong,
    /// The task legitimately wrote or changed the exam — a new feature with new
    /// tests, TDD, characterization work. The verdict proves the subject
    /// satisfies an exam Cortex partly wrote.
    ///
    /// Still a real verdict, still worth paying for: it is what every human
    /// engineer produces. It is not the same evidence as [`Strong`], and the
    /// receipt, the badge, the API field and the compliance export all carry
    /// the distinction.
    ///
    /// [`Strong`]: VerdictClass::Strong
    #[default]
    Authored,
}

/// A delivered diff, split by surface. Paths are kept so a receipt can name
/// them and a review bundle can order the exam diff first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfacePartition {
    pub subject: Vec<String>,
    pub exam: Vec<String>,
    pub incidental: Vec<String>,
}

impl SurfacePartition {
    pub fn touches_exam(&self) -> bool {
        !self.exam.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.subject.is_empty() && self.exam.is_empty() && self.incidental.is_empty()
    }
}

/// Whether the delivery honoured the class its plan declared.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClassOutcome {
    /// The delivery matches what was declared.
    Honoured(VerdictClass),
    /// Declared `strong`, delivered exam edits.
    ///
    /// **This is a contract violation, not a verdict.** Dispatch declared the
    /// class; delivery contradicted it. The run does not get to pick the
    /// weaker class after the fact, because that would make `strong` mean
    /// "strong unless it was inconvenient". It resolves as inconclusive, is not
    /// charged, and goes to a human with the exam diff shown first.
    StrongContractBroken { exam_paths: Vec<String> },
}

/// Decide whether a delivery may be graded under the class its plan declared.
pub fn resolve_class(declared: VerdictClass, partition: &SurfacePartition) -> ClassOutcome {
    match declared {
        VerdictClass::Strong if partition.touches_exam() => ClassOutcome::StrongContractBroken {
            exam_paths: partition.exam.clone(),
        },
        other => ClassOutcome::Honoured(other),
    }
}

/// Split a set of changed paths by surface.
pub fn partition(paths: &[String], facts: &EcosystemFacts) -> SurfacePartition {
    let mut out = SurfacePartition::default();
    for path in paths {
        match classify(path, facts) {
            DiffSurface::Subject => out.subject.push(path.clone()),
            DiffSurface::Exam => out.exam.push(path.clone()),
            DiffSurface::Incidental => out.incidental.push(path.clone()),
        }
    }
    out
}

/// Which surface one path belongs to.
///
/// Deterministic and ecosystem-aware. Order matters: a path is checked against
/// the exam rules first, because `tests/fixtures/Cargo.lock` is part of the
/// exam's ground truth and not an incidental lockfile.
pub fn classify(path: &str, facts: &EcosystemFacts) -> DiffSurface {
    let p = normalize(path);

    if is_exam(&p, facts) {
        return DiffSurface::Exam;
    }
    if is_incidental(&p) {
        return DiffSurface::Incidental;
    }
    DiffSurface::Subject
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_string()
}

fn segments(p: &str) -> Vec<&str> {
    p.split('/').filter(|s| !s.is_empty()).collect()
}

fn file_name(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

fn is_exam(p: &str, facts: &EcosystemFacts) -> bool {
    let segs = segments(p);
    let name = file_name(p);

    // Directories that exist to hold ground truth, at any depth.
    const EXAM_DIRS: &[&str] = &[
        "tests",
        "test",
        "__tests__",
        "spec",
        "testdata",
        "fixtures",
        "__fixtures__",
        "__snapshots__",
        "snapshots",
        "golden",
        "e2e",
        "benches",
    ];
    if segs.iter().any(|s| EXAM_DIRS.contains(s)) {
        return true;
    }

    // Filenames that are tests wherever they sit.
    if name.ends_with(".snap") || name.ends_with(".golden") {
        return true;
    }
    for infix in [".test.", ".spec.", "_test.", "_spec."] {
        if name.contains(infix) {
            return true;
        }
    }
    if name.starts_with("test_") || name.starts_with("conftest.") {
        return true;
    }

    // CI is the outermost exam: it decides which checks run at all.
    if p.starts_with(".github/workflows/") {
        return true;
    }

    // Config that sets how strict the graders are. Deliberately *not*
    // formatting config -- `rustfmt.toml` changes how code looks, not whether
    // it passes.
    const EXAM_CONFIG: &[&str] = &[
        "clippy.toml",
        "deny.toml",
        "tsconfig.json",
        ".eslintrc",
        ".eslintrc.json",
        ".eslintrc.js",
        ".eslintrc.cjs",
        "eslint.config.js",
        "eslint.config.mjs",
        "vitest.config.ts",
        "vitest.config.js",
        "jest.config.js",
        "jest.config.ts",
        "pytest.ini",
        "tox.ini",
        ".coveragerc",
        "codecov.yml",
    ];
    if EXAM_CONFIG.contains(&name) {
        return true;
    }
    if facts.has_cargo_manifest && name == "rust-toolchain.toml" {
        return true;
    }

    false
}

fn is_incidental(p: &str) -> bool {
    let segs = segments(p);
    let name = file_name(p);

    const LOCKFILES: &[&str] = &[
        "Cargo.lock",
        "package-lock.json",
        "yarn.lock",
        "pnpm-lock.yaml",
        "poetry.lock",
        "Gemfile.lock",
        "go.sum",
    ];
    if LOCKFILES.contains(&name) {
        return true;
    }

    if name.ends_with(".md") || name.ends_with(".mdx") || name.ends_with(".txt") {
        return true;
    }

    const DOC_DIRS: &[&str] = &["docs", "doc"];
    if segs.first().is_some_and(|s| DOC_DIRS.contains(s)) {
        return true;
    }

    // Build output and vendored dependencies: present in a diff only when
    // something went wrong, and never the thing being graded.
    const GENERATED_DIRS: &[&str] = &["target", "dist", "build", "node_modules", "vendor"];
    if segs.first().is_some_and(|s| GENERATED_DIRS.contains(s)) {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rust() -> EcosystemFacts {
        EcosystemFacts {
            has_cargo_manifest: true,
            has_package_json: false,
            npm_scripts: Vec::new(),
        }
    }

    fn node() -> EcosystemFacts {
        EcosystemFacts {
            has_cargo_manifest: false,
            has_package_json: true,
            npm_scripts: vec!["test".into()],
        }
    }

    #[test]
    fn the_thing_being_graded_is_separate_from_the_thing_grading_it() {
        assert_eq!(classify("src/lib.rs", &rust()), DiffSurface::Subject);
        assert_eq!(classify("tests/e2e.rs", &rust()), DiffSurface::Exam);
        assert_eq!(classify("Cargo.lock", &rust()), DiffSurface::Incidental);
    }

    #[test]
    fn ground_truth_is_exam_wherever_it_sits() {
        // These are what a check reads to decide pass or fail. A snapshot file
        // buried under `src/` still decides the grade.
        for p in [
            "src/__snapshots__/render.snap",
            "crates/api/testdata/fixture.json",
            "web/src/lib/api.test.ts",
            "web/src/lib/api.spec.tsx",
            "pkg/thing_test.go",
            "tests/fixtures/golden.txt",
            "benches/throughput.rs",
        ] {
            assert_eq!(classify(p, &node()), DiffSurface::Exam, "{p}");
        }
    }

    #[test]
    fn a_lockfile_inside_the_exam_is_still_the_exam() {
        // Order matters. `tests/fixtures/Cargo.lock` is ground truth for
        // whatever reads it, not an incidental lockfile, and reversing the
        // checks would silently reclassify it.
        assert_eq!(
            classify("tests/fixtures/Cargo.lock", &rust()),
            DiffSurface::Exam
        );
        assert_eq!(classify("Cargo.lock", &rust()), DiffSurface::Incidental);
    }

    #[test]
    fn strictness_config_grades_but_formatting_config_does_not() {
        // `clippy.toml` and `tsconfig.json` decide whether code passes.
        // `rustfmt.toml` decides how it looks. Only the first kind is an exam.
        assert_eq!(classify("clippy.toml", &rust()), DiffSurface::Exam);
        assert_eq!(classify("tsconfig.json", &node()), DiffSurface::Exam);
        assert_eq!(
            classify(".github/workflows/ci.yml", &rust()),
            DiffSurface::Exam
        );
        assert_eq!(classify("rustfmt.toml", &rust()), DiffSurface::Subject);
    }

    #[test]
    fn windows_separators_and_leading_dots_classify_the_same() {
        // Path strings reach this from git, from the worker and from a
        // Windows checkout. A classifier that depends on which is a
        // classifier that reports a different surface on a different machine.
        assert_eq!(classify("tests\\e2e.rs", &rust()), DiffSurface::Exam);
        assert_eq!(classify("./tests/e2e.rs", &rust()), DiffSurface::Exam);
    }

    #[test]
    fn a_strong_claim_over_an_edited_exam_is_not_a_verdict() {
        // The dangerous case, and the reason the class is declared before
        // delivery: dispatch said the battery would be untouched, delivery
        // touched it. Downgrading to `authored` here would make `strong` mean
        // "strong unless it was inconvenient".
        let part = partition(&["src/lib.rs".into(), "tests/e2e.rs".into()], &rust());
        match resolve_class(VerdictClass::Strong, &part) {
            ClassOutcome::StrongContractBroken { exam_paths } => {
                assert_eq!(exam_paths, vec!["tests/e2e.rs".to_string()]);
            }
            other => panic!("a broken strong contract must not resolve: {other:?}"),
        }
    }

    #[test]
    fn authoring_the_exam_is_allowed_and_stays_a_verdict() {
        // Characterization work and TDD both write the exam. Forbidding it
        // would break Phase 24.2, which is the brownfield on-ramp.
        let part = partition(
            &["src/lib.rs".into(), "tests/characterize.rs".into()],
            &rust(),
        );
        assert_eq!(
            resolve_class(VerdictClass::Authored, &part),
            ClassOutcome::Honoured(VerdictClass::Authored)
        );
    }

    #[test]
    fn a_strong_claim_over_subject_and_incidental_only_is_honoured() {
        let part = partition(
            &["src/lib.rs".into(), "Cargo.lock".into(), "README.md".into()],
            &rust(),
        );
        assert!(!part.touches_exam());
        assert_eq!(
            resolve_class(VerdictClass::Strong, &part),
            ClassOutcome::Honoured(VerdictClass::Strong)
        );
        assert_eq!(part.subject, vec!["src/lib.rs".to_string()]);
    }

    #[test]
    fn a_rust_unit_test_is_not_visible_to_path_classification() {
        // Documented limitation, asserted so it cannot be forgotten or quietly
        // assumed away. A `#[cfg(test)] mod tests` at the bottom of
        // `src/thing.rs` is exam content living at a subject path, so a
        // `strong` verdict over a Rust diff is weaker than it sounds until the
        // build graph tells us which targets are test targets.
        //
        // When that lands, this assertion should flip, and its failure is the
        // signal that it did.
        assert_eq!(classify("src/thing.rs", &rust()), DiffSurface::Subject);
    }
}
