//! PGM108 — Don't use `json` type (use `jsonb` instead)
//!
//! The `json` type stores an exact copy of the input text and must re-parse it on
//! every operation. `jsonb` stores a decomposed binary format that is significantly
//! faster for queries, supports indexing (GIN), and supports containment/existence
//! operators (`@>`, `?`, `?|`, `?&`). The only advantages of `json` are preserving
//! exact key order and duplicate keys — both rarely needed.

use crate::parser::ir::{IrNode, Located};
use crate::rules::column_type_check;
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "Column uses json type instead of jsonb";

pub(super) const EXPLAIN: &str = "PGM108 — Don't use `json` (prefer `jsonb`)\n\
         \n\
         What it detects:\n\
         A column declared as `json` in CREATE TABLE, ADD COLUMN, or ALTER COLUMN TYPE.\n\
         \n\
         Why it's problematic:\n\
         The `json` type stores an exact copy of the input text and must re-parse\n\
         it on every operation. `jsonb` stores a decomposed binary format that is\n\
         significantly faster for queries, supports indexing (GIN), and supports\n\
         containment/existence operators (`@>`, `?`, `?|`, `?&`). The only\n\
         advantages of `json` are preserving exact key order and duplicate keys\n\
         — both rarely needed.\n\
         \n\
         Example (bad):\n\
           CREATE TABLE events (payload json NOT NULL);\n\
         \n\
         Fix:\n\
           CREATE TABLE events (payload jsonb NOT NULL);";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    column_type_check::check_column_types(
        statements,
        ctx,
        rule,
        |tn| tn.name.eq_ignore_ascii_case("json"),
        |col, table, _tn| {
            format!(
                "Column '{}' on '{}' uses 'json'. Use 'jsonb' instead — \
                     it's faster, smaller, indexable, and supports containment \
                     operators. Only use 'json' if you need to preserve exact \
                     text representation or key order.",
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
    use crate::rules::{RuleId, TypeChoiceRule};
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_create_table_json_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events"))
                .with_columns(vec![ColumnDef::test("payload", "json")]),
        ))];

        let findings = RuleId::TypeChoice(TypeChoiceRule::Pgm108).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_jsonb_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events"))
                .with_columns(vec![ColumnDef::test("payload", "jsonb")]),
        ))];

        let findings = RuleId::TypeChoice(TypeChoiceRule::Pgm108).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_column_json_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("events"),
            actions: vec![AlterTableAction::AddColumn(ColumnDef::test(
                "metadata", "json",
            ))],
        }))];

        let findings = RuleId::TypeChoice(TypeChoiceRule::Pgm108).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }
}
