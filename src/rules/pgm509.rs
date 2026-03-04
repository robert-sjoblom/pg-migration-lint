//! PGM509 — Mixed-case identifiers or reserved words
//!
//! Detects table and column names that require perpetual double-quoting,
//! either because they contain uppercase characters or because they are
//! PostgreSQL reserved words.
//!
//! Key insight: `pg_query` (libpg_query) lowercases unquoted identifiers and
//! preserves case for quoted ones. So if a name contains uppercase chars, it
//! was necessarily quoted. If a name is a reserved word and the parse succeeded,
//! it was necessarily quoted.

use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity, reserved_keywords};

pub(super) const DESCRIPTION: &str =
    "Mixed-case identifier or reserved word requires double-quoting";

pub(super) const EXPLAIN: &str = "PGM509 — Mixed-case identifiers or reserved words\n\
         \n\
         What it detects:\n\
         Table and column names that will require double-quoting in every\n\
         subsequent query. This happens when a name contains uppercase\n\
         characters (was necessarily quoted in the DDL) or is a PostgreSQL\n\
         reserved word (was necessarily quoted to be used as an identifier).\n\
         \n\
         Why it matters:\n\
         Double-quoted identifiers are a persistent source of developer friction.\n\
         Every query must use the exact case and quotes, IDE autocompletion\n\
         becomes unreliable, and ORMs may generate incorrect SQL. pg_dump\n\
         output becomes harder to read and modify.\n\
         \n\
         Example (bad):\n\
           CREATE TABLE \"User\" (\"Id\" bigint, \"order\" text);\n\
           -- Every query must now use: SELECT \"Id\", \"order\" FROM \"User\";\n\
         \n\
         Example (good):\n\
           CREATE TABLE users (id bigint, order_status text);\n\
         \n\
         Fix:\n\
         Use a lowercase, non-reserved name:\n\
           CREATE TABLE users (id bigint, order_status text);\n\
         \n\
         Does NOT fire when:\n\
         - The identifier is all-lowercase and not a PostgreSQL reserved word.\n\
         - The identifier is a schema name or index name (only table and\n\
           column names are checked).\n\
         \n\
         Statements checked:\n\
         - CREATE TABLE — table name and all column names\n\
         - ALTER TABLE ... ADD COLUMN — column name\n\
         - RENAME TABLE — new name\n\
         - RENAME COLUMN — new name";

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Info;

/// Check if a name needs quoting. Returns a reason string if it does.
///
/// Reserved words are checked first so that an all-uppercase reserved word
/// like `"ORDER"` gets the more specific "PostgreSQL reserved word" label
/// rather than the generic uppercase label.
fn needs_quoting(name: &str) -> Option<&'static str> {
    if reserved_keywords::is_reserved(&name.to_lowercase()) {
        return Some("PostgreSQL reserved word");
    }
    if name.chars().any(|c| c.is_ascii_uppercase()) {
        return Some("contains uppercase characters");
    }
    None
}

/// Build a column-level finding message.
fn column_message(col_name: &str, table_display: &str, reason: &'static str) -> String {
    format!("Column '{col_name}' on table '{table_display}' requires double-quoting ({reason}).")
}

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        match &stmt.node {
            IrNode::CreateTable(ct) => {
                // Check table name
                if let Some(reason) = needs_quoting(&ct.name.name) {
                    findings.push(rule.make_finding(
                        format!(
                            "Table '{}' requires double-quoting ({reason}).",
                            ct.name.display_name(),
                        ),
                        ctx.file,
                        &stmt.span,
                    ));
                }
                // Check all column names
                for col in &ct.columns {
                    if let Some(reason) = needs_quoting(&col.name) {
                        findings.push(rule.make_finding(
                            column_message(&col.name, &ct.name.display_name(), reason),
                            ctx.file,
                            &stmt.span,
                        ));
                    }
                }
            }
            IrNode::AlterTable(at) => {
                for action in &at.actions {
                    if let AlterTableAction::AddColumn(col) = action
                        && let Some(reason) = needs_quoting(&col.name)
                    {
                        findings.push(rule.make_finding(
                            column_message(&col.name, &at.name.display_name(), reason),
                            ctx.file,
                            &stmt.span,
                        ));
                    }
                }
            }
            IrNode::RenameTable { name, new_name } => {
                if let Some(reason) = needs_quoting(new_name) {
                    findings.push(rule.make_finding(
                        format!(
                            "Table '{}' (renamed from '{}') requires double-quoting ({reason}).",
                            new_name,
                            name.display_name(),
                        ),
                        ctx.file,
                        &stmt.span,
                    ));
                }
            }
            IrNode::RenameColumn {
                table, new_name, ..
            } => {
                if let Some(reason) = needs_quoting(new_name) {
                    findings.push(rule.make_finding(
                        column_message(new_name, &table.display_name(), reason),
                        ctx.file,
                        &stmt.span,
                    ));
                }
            }
            _ => {}
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::parser::ir::*;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{located, make_ctx};
    use std::collections::HashSet;
    use std::path::PathBuf;

    // -- CreateTable tests --

    #[test]
    fn test_mixed_case_table_name_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("User"))
                .with_columns(vec![ColumnDef::test("id", "bigint").with_nullable(false)]),
        ))];

        let findings = RuleId::Pgm509.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_mixed_case_column_name_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("users")).with_columns(vec![
                ColumnDef::test("Id", "bigint").with_nullable(false),
                ColumnDef::test("name", "text"),
            ]),
        ))];

        let findings = RuleId::Pgm509.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_reserved_word_table_name_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("order"))
                .with_columns(vec![ColumnDef::test("id", "bigint").with_nullable(false)]),
        ))];

        let findings = RuleId::Pgm509.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_reserved_word_column_name_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("items")).with_columns(vec![
                ColumnDef::test("id", "bigint").with_nullable(false),
                ColumnDef::test("select", "text"),
            ]),
        ))];

        let findings = RuleId::Pgm509.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_lowercase_non_reserved_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("users")).with_columns(vec![
                ColumnDef::test("id", "bigint").with_nullable(false),
                ColumnDef::test("email", "text"),
            ]),
        ))];

        let findings = RuleId::Pgm509.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    // -- AlterTable AddColumn tests --

    #[test]
    fn test_add_column_mixed_case_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("users"),
            actions: vec![AlterTableAction::AddColumn(ColumnDef::test(
                "FirstName",
                "text",
            ))],
        }))];

        let findings = RuleId::Pgm509.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_column_reserved_word_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("users"),
            actions: vec![AlterTableAction::AddColumn(ColumnDef::test(
                "group", "text",
            ))],
        }))];

        let findings = RuleId::Pgm509.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    // -- RenameTable tests --

    #[test]
    fn test_rename_table_to_mixed_case_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::RenameTable {
            name: QualifiedName::unqualified("users"),
            new_name: "Users".to_string(),
        })];

        let findings = RuleId::Pgm509.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    // -- RenameColumn tests --

    #[test]
    fn test_rename_column_to_reserved_word_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::RenameColumn {
            table: QualifiedName::unqualified("users"),
            old_name: "user_group".to_string(),
            new_name: "select".to_string(),
        })];

        let findings = RuleId::Pgm509.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    // -- needs_quoting unit tests --

    #[test]
    fn test_needs_quoting_uppercase() {
        // "User" lowercases to "user" which is a PG reserved word
        assert_eq!(needs_quoting("User"), Some("PostgreSQL reserved word"));
        assert_eq!(
            needs_quoting("firstName"),
            Some("contains uppercase characters")
        );
        assert_eq!(needs_quoting("ID"), Some("contains uppercase characters"));
    }

    #[test]
    fn test_needs_quoting_reserved() {
        assert_eq!(needs_quoting("select"), Some("PostgreSQL reserved word"));
        assert_eq!(needs_quoting("order"), Some("PostgreSQL reserved word"));
        assert_eq!(needs_quoting("table"), Some("PostgreSQL reserved word"));
    }

    #[test]
    fn test_needs_quoting_uppercase_reserved_word() {
        // All-uppercase reserved words should get "PostgreSQL reserved word",
        // not the generic uppercase label.
        assert_eq!(needs_quoting("ORDER"), Some("PostgreSQL reserved word"));
        assert_eq!(needs_quoting("SELECT"), Some("PostgreSQL reserved word"));
        assert_eq!(needs_quoting("Table"), Some("PostgreSQL reserved word"));
    }

    #[test]
    fn test_needs_quoting_safe() {
        assert_eq!(needs_quoting("users"), None);
        assert_eq!(needs_quoting("order_status"), None);
        assert_eq!(needs_quoting("id"), None);
    }
}
