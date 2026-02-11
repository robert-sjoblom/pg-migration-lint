//! Suppression directive parsing
//!
//! Parses inline comments for suppression directives in both SQL and XML formats:
//! - SQL: `-- pgm-lint:suppress PGM001` - suppress next statement
//! - SQL: `-- pgm-lint:suppress-file PGM001,PGM003` - suppress entire file
//! - XML: `<!-- pgm-lint:suppress PGM001 -->` - suppress next statement
//! - XML: `<!-- pgm-lint:suppress-file PGM001,PGM003 -->` - suppress entire file

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
    /// Return all distinct rule IDs referenced by any suppression directive.
    pub fn rule_ids(&self) -> HashSet<&str> {
        let mut ids: HashSet<&str> = self.file_level.iter().map(|s| s.as_str()).collect();
        for rules in self.line_level.values() {
            ids.extend(rules.iter().map(|s| s.as_str()));
        }
        ids
    }

    /// Check if a rule is suppressed at a given line.
    pub fn is_suppressed(&self, rule_id: &str, statement_line: usize) -> bool {
        // Check file-level suppressions
        if self.file_level.contains(rule_id) {
            return true;
        }

        // Check line-level suppressions
        if let Some(rules) = self.line_level.get(&statement_line)
            && rules.contains(rule_id)
        {
            return true;
        }

        false
    }
}

/// The kind of suppression directive found in a comment.
enum Directive<'a> {
    /// `pgm-lint:suppress-file RULES` — file-level suppression.
    File(&'a str),
    /// `pgm-lint:suppress RULES` — next-statement suppression.
    NextStatement(&'a str),
}

/// Try to extract a suppression directive from a single line of source text.
///
/// Recognises both SQL-style (`-- pgm-lint:suppress ...`) and XML-style
/// (`<!-- pgm-lint:suppress ... -->`) single-line comments. Multi-line XML
/// comments are intentionally not supported.
///
/// Returns `Some(Directive)` with the trailing rules string if a directive
/// was found, or `None` otherwise.
fn extract_directive(trimmed: &str) -> Option<Directive<'_>> {
    // Try SQL-style comment first: "-- pgm-lint:suppress..."
    if let Some(rest) = trimmed.strip_prefix("--").map(|s| s.trim()) {
        return parse_directive_body(rest);
    }

    // Try XML-style comment: "<!-- pgm-lint:suppress... -->"
    // Must be a single-line comment: starts with "<!--" and ends with "-->".
    if let Some(inner) = trimmed.strip_prefix("<!--")
        && let Some(body) = inner.strip_suffix("-->")
    {
        let body = body.trim();
        return parse_directive_body(body);
    }

    None
}

/// Parse the body of a comment (after stripping the comment delimiters) for
/// a suppression directive.
fn parse_directive_body(body: &str) -> Option<Directive<'_>> {
    // Check file-level first (more specific prefix).
    if let Some(rules_str) = body.strip_prefix("pgm-lint:suppress-file") {
        return Some(Directive::File(rules_str.trim()));
    }
    if let Some(rules_str) = body.strip_prefix("pgm-lint:suppress") {
        return Some(Directive::NextStatement(rules_str.trim()));
    }
    None
}

/// Check whether a line is a comment (SQL or XML style).
///
/// Used when scanning forward to find the next non-comment, non-empty line
/// for next-statement suppression.
fn is_comment_line(trimmed: &str) -> bool {
    if trimmed.starts_with("--") {
        return true;
    }
    // Single-line XML comment
    if trimmed.starts_with("<!--") && trimmed.ends_with("-->") {
        return true;
    }
    false
}

/// Parse suppression comments from source text.
///
/// Supports both SQL-style (`-- pgm-lint:...`) and XML-style
/// (`<!-- pgm-lint:... -->`) single-line comments. Must be called before
/// IR parsing (operates on raw text).
pub fn parse_suppressions(source: &str) -> Suppressions {
    let mut suppressions = Suppressions::default();
    let lines: Vec<&str> = source.lines().collect();

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        if let Some(directive) = extract_directive(trimmed) {
            match directive {
                Directive::File(rules_str) => {
                    for rule_id in rules_str.split(',') {
                        let rule_id = rule_id.trim();
                        if !rule_id.is_empty() {
                            suppressions.file_level.insert(rule_id.to_string());
                        }
                    }
                }
                Directive::NextStatement(rules_str) => {
                    // Find the next non-comment, non-empty line
                    for (next_idx, next_line_raw) in lines.iter().enumerate().skip(idx + 1) {
                        let next_line = next_line_raw.trim();
                        if !next_line.is_empty() && !is_comment_line(next_line) {
                            // Statement line is 1-based
                            let statement_line = next_idx + 1;
                            let rule_set =
                                suppressions.line_level.entry(statement_line).or_default();

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

    // -----------------------------------------------------------------------
    // XML comment suppression tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_xml_comment_suppress() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog>
    <!-- pgm-lint:suppress PGM001 -->
    <changeSet id="1" author="dev">
        <createIndex indexName="idx_foo" tableName="foo">
            <column name="bar"/>
        </createIndex>
    </changeSet>
</databaseChangeLog>"#;

        let suppressions = parse_suppressions(source);
        // The next non-comment, non-empty line after line 3 is line 4 (<changeSet>)
        assert!(
            suppressions.is_suppressed("PGM001", 4),
            "PGM001 should be suppressed on line 4"
        );
        assert!(
            !suppressions.is_suppressed("PGM002", 4),
            "PGM002 should NOT be suppressed"
        );
    }

    #[test]
    fn test_xml_comment_suppress_multiple() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog>
    <!-- pgm-lint:suppress PGM001,PGM003 -->
    <changeSet id="1" author="dev">
        <sql>CREATE INDEX idx_foo ON foo(bar);</sql>
    </changeSet>
</databaseChangeLog>"#;

        let suppressions = parse_suppressions(source);
        assert!(suppressions.is_suppressed("PGM001", 4));
        assert!(suppressions.is_suppressed("PGM003", 4));
        assert!(!suppressions.is_suppressed("PGM002", 4));
    }

    #[test]
    fn test_xml_comment_suppress_file() {
        let source = r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- pgm-lint:suppress-file PGM001,PGM003 -->
<databaseChangeLog>
    <changeSet id="1" author="dev">
        <sql>CREATE INDEX idx_foo ON foo(bar);</sql>
    </changeSet>
</databaseChangeLog>"#;

        let suppressions = parse_suppressions(source);
        assert!(suppressions.is_suppressed("PGM001", 5));
        assert!(suppressions.is_suppressed("PGM003", 5));
        assert!(suppressions.is_suppressed("PGM001", 99));
        assert!(!suppressions.is_suppressed("PGM002", 5));
    }

    #[test]
    fn test_xml_comment_whitespace_variants() {
        // No space after <!--
        let source1 = "<!--pgm-lint:suppress PGM001-->\n<changeSet id=\"1\" author=\"dev\">";
        let s1 = parse_suppressions(source1);
        assert!(
            s1.is_suppressed("PGM001", 2),
            "No-space variant should parse"
        );

        // Extra spaces around directive
        let source2 = "<!--  pgm-lint:suppress PGM001  -->\n<changeSet id=\"1\" author=\"dev\">";
        let s2 = parse_suppressions(source2);
        assert!(
            s2.is_suppressed("PGM001", 2),
            "Extra-space variant should parse"
        );

        // Indented XML comment
        let source3 =
            "    <!-- pgm-lint:suppress PGM001 -->\n    <changeSet id=\"1\" author=\"dev\">";
        let s3 = parse_suppressions(source3);
        assert!(
            s3.is_suppressed("PGM001", 2),
            "Indented variant should parse"
        );

        // File-level with no space
        let source4 = "<!--pgm-lint:suppress-file PGM001-->\n<something>";
        let s4 = parse_suppressions(source4);
        assert!(
            s4.is_suppressed("PGM001", 2),
            "File-level no-space variant should parse"
        );
    }

    #[test]
    fn test_mixed_sql_and_xml_comments() {
        // A file with both SQL and XML comment styles
        let source = r#"-- pgm-lint:suppress-file PGM003
<!-- pgm-lint:suppress PGM001 -->
CREATE INDEX idx_foo ON foo(bar);
<!-- pgm-lint:suppress PGM002 -->
ALTER TABLE foo ADD COLUMN baz int;
"#;

        let suppressions = parse_suppressions(source);
        // File-level from SQL comment
        assert!(
            suppressions.is_suppressed("PGM003", 3),
            "PGM003 file-level from SQL comment"
        );
        assert!(
            suppressions.is_suppressed("PGM003", 5),
            "PGM003 file-level applies everywhere"
        );
        // Line-level from XML comment on line 2 -> targets line 3
        assert!(
            suppressions.is_suppressed("PGM001", 3),
            "PGM001 from XML comment targets line 3"
        );
        assert!(
            !suppressions.is_suppressed("PGM001", 5),
            "PGM001 should NOT apply to line 5"
        );
        // Line-level from XML comment on line 4 -> targets line 5
        assert!(
            suppressions.is_suppressed("PGM002", 5),
            "PGM002 from XML comment targets line 5"
        );
    }

    #[test]
    fn test_xml_comment_skips_other_xml_comments_to_find_statement() {
        // XML suppress comment followed by another XML comment before the statement
        let source = r#"<!-- pgm-lint:suppress PGM001 -->
<!-- This is just a regular XML comment -->
<changeSet id="1" author="dev">"#;

        let suppressions = parse_suppressions(source);
        // Should skip the regular XML comment (line 2) and target line 3
        assert!(
            suppressions.is_suppressed("PGM001", 3),
            "Should skip XML comment lines to find the next statement"
        );
        assert!(
            !suppressions.is_suppressed("PGM001", 2),
            "Should not target the intermediate comment line"
        );
    }

    #[test]
    fn test_multiline_xml_comment_not_matched() {
        // Multi-line XML comments should NOT be matched as suppressions.
        // Only single-line <!-- ... --> on one line should be parsed.
        let source = "<!--\npgm-lint:suppress PGM001\n-->\n<changeSet id=\"1\" author=\"dev\">";

        let suppressions = parse_suppressions(source);
        assert!(
            !suppressions.is_suppressed("PGM001", 4),
            "Multi-line XML comment should NOT match"
        );
    }
}
