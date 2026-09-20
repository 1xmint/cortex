use cortex_core::routing::Intent;

struct IntentPattern {
    intent: Intent,
    keywords: &'static [&'static str],
}

const PATTERNS: &[IntentPattern] = &[
    IntentPattern {
        intent: Intent::Think,
        keywords: &[
            "should we",
            "what's the best",
            "how should",
            "architecture",
            "design",
            "decide",
            "compare",
            "tradeoff",
            "evaluate",
            "approach",
        ],
    },
    IntentPattern {
        intent: Intent::Review,
        keywords: &["review", "audit", "check", "inspect", "look at"],
    },
    IntentPattern {
        intent: Intent::Explore,
        keywords: &[
            "find",
            "where is",
            "search",
            "look for",
            "explore",
            "understand",
            "what does",
            "how does",
            "show me",
        ],
    },
    IntentPattern {
        intent: Intent::Test,
        keywords: &["test", "write tests", "add tests", "run tests", "coverage"],
    },
    IntentPattern {
        intent: Intent::Refactor,
        keywords: &[
            "refactor",
            "clean up",
            "restructure",
            "rename",
            "reorganize",
        ],
    },
    IntentPattern {
        intent: Intent::Fix,
        keywords: &["fix", "bug", "broken", "error", "crash", "failing", "wrong"],
    },
    IntentPattern {
        intent: Intent::Add,
        keywords: &[
            "add",
            "create",
            "build",
            "implement",
            "make",
            "new",
            "feature",
            "set up",
            "setup",
            "install",
            "wire",
        ],
    },
];

pub fn classify_intent(input: &str) -> Option<Intent> {
    let lower = input.to_lowercase();

    let mut best: Option<(Intent, usize)> = None;

    for pattern in PATTERNS {
        for keyword in pattern.keywords {
            if let Some(pos) = lower.find(keyword) {
                match &best {
                    None => best = Some((pattern.intent, pos)),
                    Some((_, best_pos)) if pos < *best_pos => {
                        best = Some((pattern.intent, pos));
                    }
                    _ => {}
                }
            }
        }
    }

    best.map(|(intent, _)| intent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fix_intent() {
        assert_eq!(classify_intent("fix the login bug"), Some(Intent::Fix));
    }

    #[test]
    fn explore_intent() {
        assert_eq!(
            classify_intent("where is auth handled?"),
            Some(Intent::Explore)
        );
    }

    #[test]
    fn think_intent() {
        assert_eq!(
            classify_intent("should we use Redis or Postgres?"),
            Some(Intent::Think)
        );
    }

    #[test]
    fn add_intent() {
        assert_eq!(
            classify_intent("add a new endpoint for users"),
            Some(Intent::Add)
        );
    }

    #[test]
    fn review_intent() {
        assert_eq!(
            classify_intent("review the changes on this branch"),
            Some(Intent::Review)
        );
    }

    #[test]
    fn test_intent() {
        assert_eq!(
            classify_intent("write tests for the auth module"),
            Some(Intent::Test)
        );
    }

    #[test]
    fn empty_input() {
        assert_eq!(classify_intent(""), None);
    }

    #[test]
    fn earliest_match_wins() {
        assert_eq!(
            classify_intent("find the bug and fix it"),
            Some(Intent::Explore),
        );
    }
}
