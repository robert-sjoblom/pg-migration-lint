//! Suppression directive parsing
//!
//! Parses inline SQL comments for suppression directives:
//! - `-- pgm-lint:suppress PGM001` - suppress next statement
//! - `-- pgm-lint:suppress-file PGM001,PGM003` - suppress entire file

use std::collections::{HashMap, HashSet};

/// Parsed suppression directives from a single file.
#[derive(Debug, Default)]
pub struct Suppressions {
    /// Rules suppressed for the entire file.
    file_level: HashSet<String>,

    /// Rules suppressed for a specific line (the statement after the comment).
    /// Key: line number of the statement (not the comment).
    line_level: HashMap<usize, HashSet<String>>,
}

impl Suppressions {
    /// Check if a rule is suppressed at a given line.
    pub fn is_suppressed(&self, rule_id: &str, statement_line: usize) -> bool {
        // Check file-level suppressions
        if self.file_level.contains(rule_id) {
            return true;
        }

        // Check line-level suppressions
        if let Some(rules) = self.line_level.get(&statement_line) {
            if rules.contains(rule_id) {
                return true;
            }
        }

        false
    }
}

/// Parse suppression comments from SQL source text.
/// Must be called before IR parsing (operates on raw text).
pub fn parse_suppressions(source: &str) -> Suppressions {
    let mut suppressions = Suppressions::default();
    let lines: Vec<&str> = source.lines().collect();

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // File-level suppression
        if let Some(rest) = trimmed.strip_prefix("--").map(|s| s.trim()) {
            if let Some(rules_str) = rest.strip_prefix("pgm-lint:suppress-file") {
                let rules_str = rules_str.trim();
                for rule_id in rules_str.split(',') {
                    let rule_id = rule_id.trim();
                    if !rule_id.is_empty() {
                        suppressions.file_level.insert(rule_id.to_string());
                    }
                }
            }
            // Next-statement suppression
            else if let Some(rules_str) = rest.strip_prefix("pgm-lint:suppress") {
                let rules_str = rules_str.trim();
                // Find the next non-comment, non-empty line
                for (next_idx, next_line_raw) in lines.iter().enumerate().skip(idx + 1) {
                    let next_line = next_line_raw.trim();
                    if !next_line.is_empty() && !next_line.starts_with("--") {
                        // Statement line is 1-based
                        let statement_line = next_idx + 1;
                        let rule_set = suppressions
                            .line_level
                            .entry(statement_line)
                            .or_insert_with(HashSet::new);

                        for rule_id in rules_str.split(',') {
                            let rule_id = rule_id.trim();
                            if !rule_id.is_empty() {
                                rule_set.insert(rule_id.to_string());
                            }
                        }
                        break;
                    }
                }
            }
        }
    }

    suppressions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_level_suppression() {
        let source = r#"
-- pgm-lint:suppress-file PGM001,PGM003

CREATE TABLE foo (id int);
CREATE INDEX idx_foo ON foo(id);
"#;

        let suppressions = parse_suppressions(source);
        assert!(suppressions.is_suppressed("PGM001", 5));
        assert!(suppressions.is_suppressed("PGM003", 5));
        assert!(!suppressions.is_suppressed("PGM002", 5));
    }

    #[test]
    fn test_next_statement_suppression() {
        let source = r#"
CREATE TABLE foo (id int);

-- pgm-lint:suppress PGM001
CREATE INDEX idx_foo ON foo(id);
"#;

        let suppressions = parse_suppressions(source);
        assert!(suppressions.is_suppressed("PGM001", 5));
        assert!(!suppressions.is_suppressed("PGM001", 2));
    }

    #[test]
    fn test_multiple_rules_next_statement() {
        let source = r#"
-- pgm-lint:suppress PGM001, PGM002
CREATE INDEX idx_foo ON foo(id);
"#;

        let suppressions = parse_suppressions(source);
        assert!(suppressions.is_suppressed("PGM001", 3));
        assert!(suppressions.is_suppressed("PGM002", 3));
    }
}
