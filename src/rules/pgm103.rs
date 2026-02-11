//! PGM103 — Don't use `char(n)`
//!
//! Detects columns declared as `char(n)` (which pg_query canonicalizes to `bpchar`).
//! The `char(n)` type pads with spaces, wastes storage, and is no faster than
//! `text` or `varchar` in PostgreSQL.

use crate::parser::ir::{IrNode, Located};
use crate::rules::column_type_check;
use crate::rules::{Finding, LintContext, Rule, Severity};

/// Rule that flags the use of `char(n)`.
pub struct Pgm103;

impl Rule for Pgm103 {
    fn id(&self) -> &'static str {
        "PGM103"
    }

    fn default_severity(&self) -> Severity {
        Severity::Minor
    }

    fn description(&self) -> &'static str {
        "Column uses char(n) type"
    }

    fn explain(&self) -> &'static str {
        "PGM103 — Don't use `char(n)`\n\
         \n\
         What it detects:\n\
         A column declared as `char(n)` or `character(n)`.\n\
         \n\
         Why it's problematic:\n\
         In PostgreSQL, `char(n)` pads values with trailing spaces to fill\n\
         the declared length. This wastes storage, causes surprising equality\n\
         semantics (trailing spaces are ignored in comparisons but present\n\
         in the stored data), and is no faster than `text` or `varchar`.\n\
         \n\
         The PostgreSQL documentation itself recommends using `text` or\n\
         `varchar` instead: \"There is no performance difference among these\n\
         three types\" and \"In most situations text or character varying\n\
         should be used instead.\"\n\
         \n\
         Example (bad):\n\
           CREATE TABLE countries (code char(2) NOT NULL);\n\
         \n\
         Fix:\n\
           CREATE TABLE countries (code text NOT NULL);\n\
           -- or: code varchar(2) NOT NULL"
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        column_type_check::check_column_types(
            statements,
            ctx,
            self.id(),
            self.default_severity(),
            |tn| tn.name.eq_ignore_ascii_case("bpchar"),
            |col, table, tn| {
                let display = if let Some(&n) = tn.modifiers.first() {
                    format!("char({})", n)
                } else {
                    "char".to_string()
                };
                format!(
                    "Column '{}' on '{}' uses '{}'. The char(n) type pads with \
                     spaces, wastes storage, and is no faster than text or varchar \
                     in PostgreSQL. Use text or varchar instead.",
                    col,
                    table.display_name(),
                    display,
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
    fn test_char_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("countries"),
            columns: vec![ColumnDef {
                name: "code".to_string(),
                type_name: TypeName::simple("bpchar"),
                nullable: false,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            }],
            constraints: vec![],
            temporary: false,
        }))];

        let findings = Pgm103.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM103");
        assert_eq!(findings[0].severity, Severity::Minor);
        assert!(findings[0].message.contains("code"));
        assert!(findings[0].message.contains("char"));
    }

    #[test]
    fn test_char_n_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("countries"),
            columns: vec![ColumnDef {
                name: "code".to_string(),
                type_name: TypeName::with_modifiers("bpchar", vec![2]),
                nullable: false,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            }],
            constraints: vec![],
            temporary: false,
        }))];

        let findings = Pgm103.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("char(2)"));
    }

    #[test]
    fn test_text_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("countries"),
            columns: vec![ColumnDef {
                name: "code".to_string(),
                type_name: TypeName::simple("text"),
                nullable: false,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            }],
            constraints: vec![],
            temporary: false,
        }))];

        let findings = Pgm103.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_varchar_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("countries"),
            columns: vec![ColumnDef {
                name: "code".to_string(),
                type_name: TypeName::with_modifiers("varchar", vec![2]),
                nullable: false,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            }],
            constraints: vec![],
            temporary: false,
        }))];

        let findings = Pgm103.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
