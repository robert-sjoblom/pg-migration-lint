//! PGM004 — Table without primary key
//!
//! Detects `CREATE TABLE` statements (non-temporary) that result in a table
//! without a primary key. Checks the catalog state AFTER the entire file is
//! processed, so `ALTER TABLE ... ADD PRIMARY KEY` later in the same file
//! avoids a false positive.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

/// Rule that flags tables created without a primary key.
pub struct Pgm004;

impl Rule for Pgm004 {
    fn id(&self) -> &'static str {
        "PGM004"
    }

    fn default_severity(&self) -> Severity {
        Severity::Major
    }

    fn description(&self) -> &'static str {
        "Table without primary key"
    }

    fn explain(&self) -> &'static str {
        "PGM004 — Table without primary key\n\
         \n\
         What it detects:\n\
         A CREATE TABLE statement (non-temporary) that does not define a\n\
         PRIMARY KEY constraint, and no ALTER TABLE ... ADD PRIMARY KEY\n\
         follows in the same file.\n\
         \n\
         Why it's dangerous:\n\
         Tables without primary keys:\n\
         - Cannot be reliably targeted by logical replication.\n\
         - May cause issues with ORMs that require a PK for identity.\n\
         - Make it harder to deduplicate or reference specific rows.\n\
         - Are a strong code smell indicating incomplete schema design.\n\
         \n\
         Example (bad):\n\
           CREATE TABLE events (event_type text, payload jsonb);\n\
         \n\
         Fix:\n\
           CREATE TABLE events (\n\
             id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,\n\
             event_type text,\n\
             payload jsonb\n\
           );\n\
         \n\
         Note: Temporary tables are excluded. If PGM005 fires (UNIQUE NOT NULL\n\
         used instead of PK), PGM004 does NOT fire for the same table."
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

                // Post-file check: look at catalog_after to see if a PK was added.
                let table_state = ctx.catalog_after.get_table(table_key);
                let has_pk = table_state.map(|t| t.has_primary_key).unwrap_or(false);

                if !has_pk {
                    // Check for PGM005 condition: UNIQUE NOT NULL substitute.
                    // If PGM005 would fire, suppress PGM004.
                    let has_unique_not_null = table_state
                        .map(|t| t.has_unique_not_null())
                        .unwrap_or(false);

                    if !has_unique_not_null {
                        findings.push(Finding {
                            rule_id: self.id().to_string(),
                            severity: self.default_severity(),
                            message: format!("Table '{}' has no primary key.", ct.name),
                            file: ctx.file.clone(),
                            start_line: stmt.span.start_line,
                            end_line: stmt.span.end_line,
                        });
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
    use crate::catalog::builder::CatalogBuilder;
    use crate::catalog::Catalog;
    use crate::parser::ir::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn make_ctx<'a>(
        before: &'a Catalog,
        after: &'a Catalog,
        file: &'a PathBuf,
        created: &'a HashSet<String>,
    ) -> LintContext<'a> {
        LintContext {
            catalog_before: before,
            catalog_after: after,
            tables_created_in_change: created,
            run_in_transaction: true,
            is_down: false,
            file,
        }
    }

    fn located(node: IrNode) -> Located<IrNode> {
        Located {
            node,
            span: SourceSpan {
                start_line: 1,
                end_line: 1,
                start_offset: 0,
                end_offset: 0,
            },
        }
    }

    #[test]
    fn test_no_pk_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("events", |t| {
                t.column("event_type", "text", true)
                    .column("payload", "jsonb", true);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("events"),
            columns: vec![
                ColumnDef {
                    name: "event_type".to_string(),
                    type_name: TypeName::simple("text"),
                    nullable: true,
                    default_expr: None,
                    is_inline_pk: false,
                    is_serial: false,
                },
                ColumnDef {
                    name: "payload".to_string(),
                    type_name: TypeName::simple("jsonb"),
                    nullable: true,
                    default_expr: None,
                    is_inline_pk: false,
                    is_serial: false,
                },
            ],
            constraints: vec![],
            temporary: false,
        }))];

        let findings = Pgm004.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM004");
        assert_eq!(findings[0].severity, Severity::Major);
        assert!(findings[0].message.contains("events"));
    }

    #[test]
    fn test_with_pk_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("events", |t| {
                t.column("id", "bigint", false)
                    .pk(&["id"])
                    .column("event_type", "text", true);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("events"),
            columns: vec![
                ColumnDef {
                    name: "id".to_string(),
                    type_name: TypeName::simple("bigint"),
                    nullable: false,
                    default_expr: None,
                    is_inline_pk: true,
                    is_serial: false,
                },
                ColumnDef {
                    name: "event_type".to_string(),
                    type_name: TypeName::simple("text"),
                    nullable: true,
                    default_expr: None,
                    is_inline_pk: false,
                    is_serial: false,
                },
            ],
            constraints: vec![TableConstraint::PrimaryKey {
                columns: vec!["id".to_string()],
            }],
            temporary: false,
        }))];

        let findings = Pgm004.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_pk_added_later_in_file_no_finding() {
        let before = Catalog::new();
        // catalog_after has PK because replay already processed the ALTER TABLE
        let after = CatalogBuilder::new()
            .table("events", |t| {
                t.column("id", "integer", false)
                    .column("name", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("events"),
            columns: vec![
                ColumnDef {
                    name: "id".to_string(),
                    type_name: TypeName::simple("integer"),
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
            constraints: vec![],
            temporary: false,
        }))];

        let findings = Pgm004.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_temp_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("tmp_data"),
            columns: vec![ColumnDef {
                name: "val".to_string(),
                type_name: TypeName::simple("text"),
                nullable: true,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            }],
            constraints: vec![],
            temporary: true,
        }))];

        let findings = Pgm004.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_unique_not_null_suppresses_pgm004() {
        let before = Catalog::new();
        // Table has UNIQUE NOT NULL but no PK — PGM005 fires, PGM004 should not.
        let after = CatalogBuilder::new()
            .table("events", |t| {
                t.column("email", "text", false)
                    .unique("uk_email", &["email"]);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("events"),
            columns: vec![ColumnDef {
                name: "email".to_string(),
                type_name: TypeName::simple("text"),
                nullable: false,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            }],
            constraints: vec![TableConstraint::Unique {
                name: Some("uk_email".to_string()),
                columns: vec!["email".to_string()],
            }],
            temporary: false,
        }))];

        let findings = Pgm004.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
