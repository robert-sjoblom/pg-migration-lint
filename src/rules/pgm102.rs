//! PGM102 — Don't use `timestamp(0)` or `timestamptz(0)`
//!
//! Detects timestamp columns with precision 0. Precision 0 causes rounding,
//! not truncation — a value of '23:59:59.9' rounds to the next day.
//! Use full precision and format on output instead.

use crate::parser::ir::{IrNode, Located};
use crate::rules::column_type_check;
use crate::rules::{Finding, LintContext, Rule, Severity};

/// Rule that flags the use of `timestamp(0)` or `timestamptz(0)`.
pub struct Pgm102;

impl Rule for Pgm102 {
    fn id(&self) -> &'static str {
        "PGM102"
    }

    fn default_severity(&self) -> Severity {
        Severity::Minor
    }

    fn description(&self) -> &'static str {
        "Column uses timestamp or timestamptz with precision 0"
    }

    fn explain(&self) -> &'static str {
        "PGM102 — Don't use `timestamp(0)` or `timestamptz(0)`\n\
         \n\
         What it detects:\n\
         A column declared as `timestamp(0)` or `timestamptz(0)`.\n\
         \n\
         Why it's problematic:\n\
         Precision 0 causes PostgreSQL to round the fractional seconds,\n\
         not truncate them. A value of '2024-12-31 23:59:59.9' rounds to\n\
         '2025-01-01 00:00:00', which is the next day (and potentially the\n\
         next year). This can cause subtle bugs in date-boundary logic,\n\
         audit trails, and ordering.\n\
         \n\
         The default precision (6 microseconds) is almost always sufficient.\n\
         If you need to reduce storage or display precision, format the\n\
         output rather than constraining the stored value.\n\
         \n\
         Example (bad):\n\
           CREATE TABLE events (created_at timestamptz(0));\n\
         \n\
         Fix:\n\
           CREATE TABLE events (created_at timestamptz);"
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        column_type_check::check_column_types(
            statements,
            ctx,
            self.id(),
            self.default_severity(),
            |tn| (tn.name == "timestamp" || tn.name == "timestamptz") && tn.modifiers == [0],
            |col, table, tn| {
                format!(
                    "Column '{}' on '{}' uses '{}(0)'. Precision 0 causes \
                     rounding, not truncation \u{2014} a value of '23:59:59.9' \
                     rounds to the next day. Use full precision and format on \
                     output instead.",
                    col, table, tn.name,
                )
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::parser::ir::*;
    use crate::rules::test_helpers::{located, make_ctx};
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_timestamptz_0_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("events"),
            columns: vec![ColumnDef {
                name: "created_at".to_string(),
                type_name: TypeName::with_modifiers("timestamptz", vec![0]),
                nullable: true,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            }],
            constraints: vec![],
            temporary: false,
        }))];

        let findings = Pgm102.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM102");
        assert_eq!(findings[0].severity, Severity::Minor);
        assert!(findings[0].message.contains("created_at"));
        assert!(findings[0].message.contains("timestamptz(0)"));
    }

    #[test]
    fn test_timestamp_0_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("events"),
            columns: vec![ColumnDef {
                name: "ts".to_string(),
                type_name: TypeName::with_modifiers("timestamp", vec![0]),
                nullable: true,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            }],
            constraints: vec![],
            temporary: false,
        }))];

        let findings = Pgm102.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("timestamp(0)"));
    }

    #[test]
    fn test_timestamptz_3_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("events"),
            columns: vec![ColumnDef {
                name: "created_at".to_string(),
                type_name: TypeName::with_modifiers("timestamptz", vec![3]),
                nullable: true,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            }],
            constraints: vec![],
            temporary: false,
        }))];

        let findings = Pgm102.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_timestamptz_no_modifier_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("events"),
            columns: vec![ColumnDef {
                name: "created_at".to_string(),
                type_name: TypeName::simple("timestamptz"),
                nullable: true,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            }],
            constraints: vec![],
            temporary: false,
        }))];

        let findings = Pgm102.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
