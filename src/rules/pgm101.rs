//! PGM101 — Don't use `timestamp` (without time zone)
//!
//! Detects columns declared as `timestamp` (i.e. `timestamp without time zone`).
//! This type stores no timezone context, making values ambiguous.
//! Use `timestamptz` (timestamp with time zone) instead.

use crate::parser::ir::{IrNode, Located};
use crate::rules::column_type_check;
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "Column uses timestamp without time zone";

pub(super) const EXPLAIN: &str = "PGM101 — Don't use `timestamp` (without time zone)\n\
         \n\
         What it detects:\n\
         A column declared as `timestamp` (which PostgreSQL interprets as\n\
         `timestamp without time zone`).\n\
         \n\
         Why it's problematic:\n\
         `timestamp` (without time zone) stores a date/time value with no\n\
         timezone context. This makes the stored values ambiguous — they could\n\
         represent any timezone, and PostgreSQL performs no conversion on\n\
         input or output. When servers, clients, or applications use different\n\
         timezones, this leads to subtle, hard-to-debug data corruption.\n\
         \n\
         `timestamptz` (timestamp with time zone) stores values as UTC\n\
         internally and converts to the session's timezone on output. This\n\
         ensures unambiguous points in time.\n\
         \n\
         Example (bad):\n\
           CREATE TABLE events (created_at timestamp NOT NULL);\n\
         \n\
         Fix:\n\
           CREATE TABLE events (created_at timestamptz NOT NULL);";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    column_type_check::check_column_types(
        statements,
        ctx,
        rule,
        |tn| tn.name.eq_ignore_ascii_case("timestamp"),
        |col, table, _tn| {
            format!(
                "Column '{}' on '{}' uses 'timestamp without time zone'. \
                     Use 'timestamptz' (timestamp with time zone) instead to \
                     store unambiguous points in time.",
                col,
                table.display_name(),
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::parser::ir::*;
    use crate::rules::test_helpers::{located, make_ctx};
    use crate::rules::{RuleId, TypeAntiPatternRule};
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_create_table_timestamp_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events"))
                .with_columns(vec![ColumnDef::test("created_at", "timestamp")]),
        ))];

        let findings = RuleId::TypeAntiPattern(TypeAntiPatternRule::Pgm101).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_timestamptz_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events"))
                .with_columns(vec![ColumnDef::test("created_at", "timestamptz")]),
        ))];

        let findings = RuleId::TypeAntiPattern(TypeAntiPatternRule::Pgm101).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_column_timestamp_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("events"),
            actions: vec![AlterTableAction::AddColumn(ColumnDef::test(
                "updated_at",
                "timestamp",
            ))],
        }))];

        let findings = RuleId::TypeAntiPattern(TypeAntiPatternRule::Pgm101).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_alter_column_type_timestamp_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("events"),
            actions: vec![AlterTableAction::AlterColumnType {
                column_name: "created_at".to_string(),
                new_type: TypeName::simple("timestamp"),
                old_type: None,
            }],
        }))];

        let findings = RuleId::TypeAntiPattern(TypeAntiPatternRule::Pgm101).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }
}
