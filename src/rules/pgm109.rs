//! PGM109 — Floating-point column type
//!
//! IEEE 754 floating-point types (`real`/`float4`, `double precision`/`float8`)
//! suffer from precision issues (`0.1 + 0.2 ≠ 0.3`). For money, quantities,
//! measurements, or any domain requiring exact decimal values, `numeric` is the
//! correct choice. Floating-point errors compound in aggregations and can cause
//! silent data corruption.

use crate::parser::ir::{IrNode, Located};
use crate::rules::column_type_check;
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "Column uses floating-point type instead of numeric";

pub(super) const EXPLAIN: &str = "PGM109 — Floating-point column type\n\
         \n\
         What it detects:\n\
         A column declared as `real`, `float4`, `double precision`, `float8`, or\n\
         `float` in CREATE TABLE, ADD COLUMN, or ALTER COLUMN TYPE.\n\
         \n\
         Why it's problematic:\n\
         IEEE 754 floating-point types suffer from precision issues — for example,\n\
         `0.1 + 0.2 ≠ 0.3`. For money, quantities, measurements, or any domain\n\
         where exact decimal values matter, `numeric`/`decimal` is correct.\n\
         Floating-point errors compound in aggregations and can cause silent data\n\
         corruption.\n\
         \n\
         Example (bad):\n\
           CREATE TABLE products (price double precision NOT NULL);\n\
         \n\
         Fix:\n\
           CREATE TABLE products (price numeric(10,2) NOT NULL);";

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Minor;

/// Map pg_query canonical names to human-readable display names.
fn display_name(canonical: &str) -> &str {
    match canonical {
        "float4" => "real",
        "float8" => "double precision",
        _ => canonical,
    }
}

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    column_type_check::check_column_types(
        statements,
        ctx,
        rule,
        |tn| tn.name.eq_ignore_ascii_case("float4") || tn.name.eq_ignore_ascii_case("float8"),
        |col, table, tn| {
            let display = display_name(&tn.name);
            format!(
                "Column '{}' on '{}' uses '{}'. Floating-point types have \
                 precision issues (0.1 + 0.2 ≠ 0.3). Use numeric for exact values.",
                col,
                table.display_name(),
                display,
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::parser::ir::*;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{located, make_ctx};
    use rstest::rstest;
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[rstest]
    #[case::create_table_float8(
        "create_table_float8",
        "migrations/001.sql",
        located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("products"))
                .with_columns(vec![ColumnDef::test("price", "float8")]),
        ))
    )]
    #[case::create_table_float4(
        "create_table_float4",
        "migrations/001.sql",
        located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("sensors"))
                .with_columns(vec![ColumnDef::test("reading", "float4")]),
        ))
    )]
    #[case::add_column_float8(
        "add_column_float8",
        "migrations/002.sql",
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("products"),
            actions: vec![AlterTableAction::AddColumn(ColumnDef::test("score", "float8"))],
        }))
    )]
    #[case::alter_column_type_float4(
        "alter_column_type_float4",
        "migrations/003.sql",
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("sensors"),
            actions: vec![AlterTableAction::AlterColumnType {
                column_name: "reading".to_string(),
                new_type: TypeName { name: "float4".to_string(), modifiers: vec![] },
                old_type: None,
            }],
        }))
    )]
    fn fires(#[case] name: &str, #[case] migration_file: &str, #[case] stmt: Located<IrNode>) {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from(migration_file);
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let findings = RuleId::Pgm109.check(&[stmt], &ctx);
        insta::assert_yaml_snapshot!(format!("fires_{name}"), findings);
    }

    #[rstest]
    #[case::numeric("products", "price", "numeric")]
    #[case::integer("counters", "count", "int4")]
    fn no_finding(#[case] table: &str, #[case] column: &str, #[case] col_type: &str) {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified(table))
                .with_columns(vec![ColumnDef::test(column, col_type)]),
        ))];

        let findings = RuleId::Pgm109.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
