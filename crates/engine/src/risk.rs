use cortex_core::routing::RiskLevel;

struct Pattern {
    level: RiskLevel,
    keywords: &'static [&'static str],
}

const PATTERNS: &[Pattern] = &[
    Pattern {
        level: RiskLevel::Critical,
        keywords: &[
            "auth",
            "credential",
            "secret",
            ".env",
            "token",
            "password",
            "encrypt",
            "certificate",
            ".pem",
            ".key",
        ],
    },
    Pattern {
        level: RiskLevel::High,
        keywords: &[
            "billing",
            "payment",
            "migration",
            "deploy",
            "ci/cd",
            ".github/workflows",
            "security",
            "permission",
            "schema.prisma",
            "schema.sql",
            "api_contract",
            "openapi",
        ],
    },
    Pattern {
        level: RiskLevel::Medium,
        keywords: &[
            "test", "spec", ".test.", ".spec.", "shared", "util", "lib/", "config", ".config.",
        ],
    },
    Pattern {
        level: RiskLevel::Low,
        keywords: &[
            "readme",
            ".md",
            "docs/",
            "comment",
            "format",
            "lint",
            ".prettierrc",
            "changelog",
        ],
    },
];

pub fn classify_risk(paths: &[&str]) -> RiskLevel {
    let mut highest = RiskLevel::Low;

    for path in paths {
        let lower = path.to_lowercase();
        for pattern in PATTERNS {
            if pattern.keywords.iter().any(|kw| lower.contains(kw)) && pattern.level > highest {
                highest = pattern.level;
            }
        }
    }

    highest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_files_are_critical() {
        assert_eq!(
            classify_risk(&["src/auth/middleware.ts"]),
            RiskLevel::Critical
        );
    }

    #[test]
    fn migration_files_are_high() {
        assert_eq!(
            classify_risk(&["db/migration/001_users.sql"]),
            RiskLevel::High
        );
    }

    #[test]
    fn test_files_are_medium() {
        assert_eq!(classify_risk(&["src/utils.test.ts"]), RiskLevel::Medium);
    }

    #[test]
    fn docs_are_low() {
        assert_eq!(classify_risk(&["README.md"]), RiskLevel::Low);
    }

    #[test]
    fn highest_risk_wins() {
        assert_eq!(
            classify_risk(&["README.md", "src/auth/login.ts"]),
            RiskLevel::Critical,
        );
    }

    #[test]
    fn empty_paths_are_low() {
        assert_eq!(classify_risk(&[]), RiskLevel::Low);
    }
}
