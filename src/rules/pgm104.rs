//! PGM104 — Don't use `money` type
//!
//! Detects columns declared as `money`. The money type depends on the
//! `lc_monetary` locale setting, making it unreliable across environments.
//! Use `numeric(p,s)` instead.

use crate::parser::ir::{IrNode, Located};
use crate::rules::column_type_check;
use crate::rules::{Finding, LintContext, Rule, Severity};

/// Rule that flags the use of the `money` type.
pub struct Pgm104;

impl Rule for Pgm104 {
    fn id(&self) -> &'static str {
        "PGM104"
    }

    fn default_severity(&self) -> Severity {
        Severity::Minor
    }

    fn description(&self) -> &'static str {
        "Column uses the money type"
    }

    fn explain(&self) -> &'static str {
        "PGM104 — Don't use `money` type\n\
         \n\
         What it detects:\n\
         A column declared as `money`.\n\
         \n\
         Why it's problematic:\n\
         The `money` type formats its output (and parses input) according\n\
         to the `lc_monetary` locale setting on the PostgreSQL server. This\n\
         means the same stored value can appear differently on different\n\
         servers, and importing/exporting data between servers with different\n\
         locale settings can corrupt values. It also has limited precision\n\
         (fixed to the locale's currency format) and poor interoperability\n\
         with other numeric types.\n\
         \n\
         `numeric(p,s)` is the recommended alternative for monetary values.\n\
         It has arbitrary precision, no locale dependency, and well-defined\n\
         arithmetic behavior.\n\
         \n\
         Example (bad):\n\
           CREATE TABLE orders (total money NOT NULL);\n\
         \n\
         Fix:\n\
           CREATE TABLE orders (total numeric(12,2) NOT NULL);"
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        column_type_check::check_column_types(
            statements,
            ctx,
            self.id(),
            self.default_severity(),
            |tn| tn.name.eq_ignore_ascii_case("money"),
            |col, table, _tn| {
                format!(
                    "Column '{}' on '{}' uses the 'money' type. The money type \
                     depends on the lc_monetary locale setting, making it \
                     unreliable across environments. Use numeric(p,s) instead.",
                    col,
                    table.display_name(),
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
    fn test_money_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("orders"),
            columns: vec![ColumnDef {
                name: "total".to_string(),
                type_name: TypeName::simple("money"),
                nullable: false,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            }],
            constraints: vec![],
            temporary: false,
        }))];

        let findings = Pgm104.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_numeric_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("orders"),
            columns: vec![ColumnDef {
                name: "total".to_string(),
                type_name: TypeName::with_modifiers("numeric", vec![12, 2]),
                nullable: false,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            }],
            constraints: vec![],
            temporary: false,
        }))];

        let findings = Pgm104.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_column_money_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(ColumnDef {
                name: "discount".to_string(),
                type_name: TypeName::simple("money"),
                nullable: true,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            })],
        }))];

        let findings = Pgm104.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }
}
