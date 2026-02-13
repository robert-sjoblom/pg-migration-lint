//! PGM005 — `UNIQUE NOT NULL` used instead of primary key
//!
//! Detects tables that have no primary key but have at least one UNIQUE
//! constraint where all constituent columns are NOT NULL. This is functionally
//! equivalent to a PK but less conventional.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

/// Rule that flags tables using UNIQUE NOT NULL instead of a proper PRIMARY KEY.
pub struct Pgm005;

impl Rule for Pgm005 {
    fn id(&self) -> &'static str {
        "PGM005"
    }

    fn default_severity(&self) -> Severity {
        Severity::Info
    }

    fn description(&self) -> &'static str {
        "UNIQUE NOT NULL used instead of PRIMARY KEY"
    }

    fn explain(&self) -> &'static str {
        "PGM005 — UNIQUE NOT NULL used instead of PRIMARY KEY\n\
         \n\
         What it detects:\n\
         A table that has no PRIMARY KEY but has at least one UNIQUE constraint\n\
         where all constituent columns are NOT NULL. This combination is\n\
         functionally equivalent to a PK.\n\
         \n\
         Why it matters:\n\
         While UNIQUE NOT NULL is functionally equivalent to PRIMARY KEY,\n\
         using PRIMARY KEY is more conventional and explicit. Tools, ORMs,\n\
         and database administrators expect PK as the standard way to\n\
         identify rows. Using UNIQUE NOT NULL may confuse readers and\n\
         prevent some tools from auto-detecting the identity column.\n\
         \n\
         Example (flagged):\n\
           CREATE TABLE users (\n\
             email text NOT NULL UNIQUE,\n\
             name text\n\
           );\n\
         \n\
         Fix:\n\
           CREATE TABLE users (\n\
             email text PRIMARY KEY,\n\
             name text\n\
           );\n\
         \n\
         Note: When PGM005 fires, PGM004 (table without PK) does NOT fire\n\
         for the same table, since the situation is already flagged."
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        let mut findings = Vec::new();

        for stmt in statements {
            if let IrNode::CreateTable(ref ct) = stmt.node {
                // Skip temporary tables.
                if ct.temporary {
                    continue;
                }

                let table_key = ct.name.catalog_key();
                let table_state = ctx.catalog_after.get_table(table_key);

                let has_pk = table_state.map(|t| t.has_primary_key).unwrap_or(false);

                if !has_pk {
                    let has_unique_not_null = table_state
                        .map(|t| t.has_unique_not_null())
                        .unwrap_or(false);

                    if has_unique_not_null {
                        findings.push(self.make_finding(
                            format!(
                                "Table '{}' uses UNIQUE NOT NULL instead of PRIMARY KEY. \
                                 Functionally equivalent but PRIMARY KEY is conventional \
                                 and more explicit.",
                                ct.name.display_name()
                            ),
                            ctx.file,
                            &stmt.span,
                        ));
                    }
                }
            }
        }

        findings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::test_helpers::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_unique_not_null_no_pk_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("users", |t| {
                t.column("email", "text", false)
                    .column("name", "text", true)
                    .unique("uk_email", &["email"]);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("users"),
            columns: vec![
                ColumnDef {
                    name: "email".to_string(),
                    type_name: TypeName::simple("text"),
                    nullable: false,
                    default_expr: None,
                    is_inline_pk: false,
                    is_serial: false,
                },
                ColumnDef {
                    name: "name".to_string(),
                    type_name: TypeName::simple("text"),
                    nullable: true,
                    default_expr: None,
                    is_inline_pk: false,
                    is_serial: false,
                },
            ],
            constraints: vec![TableConstraint::Unique {
                name: Some("uk_email".to_string()),
                columns: vec!["email".to_string()],
            }],
            temporary: false,
        }))];

        let findings = Pgm005.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM005");
        assert_eq!(findings[0].severity, Severity::Info);
        assert!(findings[0].message.contains("users"));
    }

    #[test]
    fn test_with_pk_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("users", |t| {
                t.column("id", "integer", false)
                    .column("email", "text", false)
                    .pk(&["id"])
                    .unique("uk_email", &["email"]);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("users"),
            columns: vec![
                ColumnDef {
                    name: "id".to_string(),
                    type_name: TypeName::simple("integer"),
                    nullable: false,
                    default_expr: None,
                    is_inline_pk: true,
                    is_serial: false,
                },
                ColumnDef {
                    name: "email".to_string(),
                    type_name: TypeName::simple("text"),
                    nullable: false,
                    default_expr: None,
                    is_inline_pk: false,
                    is_serial: false,
                },
            ],
            constraints: vec![
                TableConstraint::PrimaryKey {
                    columns: vec!["id".to_string()],
                },
                TableConstraint::Unique {
                    name: Some("uk_email".to_string()),
                    columns: vec!["email".to_string()],
                },
            ],
            temporary: false,
        }))];

        let findings = Pgm005.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
