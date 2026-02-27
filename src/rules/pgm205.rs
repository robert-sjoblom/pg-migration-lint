//! PGM205 — `DROP SCHEMA ... CASCADE`
//!
//! Detects `DROP SCHEMA ... CASCADE`. This is the most destructive single DDL
//! statement in PostgreSQL — it silently drops every object in the schema:
//! tables, views, sequences, functions, types, and indexes.
//!
//! Unlike other destructive rules, this **always fires** when CASCADE is
//! present, regardless of catalog state. The catalog only tracks tables from
//! parsed migrations, so there may be objects we don't know about. Known
//! affected tables are listed in the message for context.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "DROP SCHEMA CASCADE";

pub(super) const EXPLAIN: &str = "PGM205 — DROP SCHEMA CASCADE\n\
         \n\
         What it detects:\n\
         A DROP SCHEMA ... CASCADE statement.\n\
         \n\
         Why it matters:\n\
         DROP SCHEMA CASCADE drops every object in the schema — tables, views,\n\
         sequences, functions, types, and indexes — in a single statement. It is\n\
         the most destructive single DDL statement in PostgreSQL and cannot be\n\
         selectively undone.\n\
         \n\
         Unlike DROP TABLE CASCADE (which only removes objects that depend on one\n\
         table), DROP SCHEMA CASCADE destroys the entire namespace and everything\n\
         in it.\n\
         \n\
         Example:\n\
           DROP SCHEMA myschema CASCADE;\n\
         \n\
         This silently drops every table, view, function, sequence, and type\n\
         defined in 'myschema'.\n\
         \n\
         Recommended approach:\n\
         1. Enumerate all objects in the schema before dropping.\n\
         2. Explicitly drop or migrate each object in separate migration steps.\n\
         3. Use plain DROP SCHEMA (without CASCADE) so PostgreSQL will error\n\
            if the schema is non-empty.\n\
         \n\
         This rule is CRITICAL severity because CASCADE silently destroys\n\
         every object in the schema.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::DropSchema(ref ds) = stmt.node {
            if !ds.cascade {
                continue;
            }

            // Find known affected tables from catalog_before
            let prefix = format!("{}.", ds.schema_name);
            let mut affected_tables: Vec<String> = ctx
                .catalog_before
                .tables()
                .filter(|t| t.name.starts_with(&prefix))
                .map(|t| t.display_name.clone())
                .collect();
            affected_tables.sort();

            let message = if affected_tables.is_empty() {
                format!(
                    "DROP SCHEMA '{}' CASCADE drops every object in the schema \
                     — tables, views, sequences, functions, and types. \
                     This is irreversible.",
                    ds.schema_name
                )
            } else {
                format!(
                    "DROP SCHEMA '{}' CASCADE drops every object in the schema \
                     — tables, views, sequences, functions, and types. \
                     This is irreversible. Known affected tables: {}.",
                    ds.schema_name,
                    affected_tables.join(", ")
                )
            };

            findings.push(rule.make_finding(message, ctx.file, &stmt.span));
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{located, make_ctx};
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn rule_id() -> RuleId {
        RuleId::Pgm205
    }

    #[test]
    fn test_drop_schema_cascade_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/016.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            DropSchema::test("myschema").with_cascade(true).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_schema_cascade_with_known_tables_lists_them() {
        let before = CatalogBuilder::new()
            .table("myschema.orders", |t| {
                t.column("id", "bigint", false).pk(&["id"]);
            })
            .table("myschema.customers", |t| {
                t.column("id", "bigint", false).pk(&["id"]);
            })
            .build();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/016.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            DropSchema::test("myschema").with_cascade(true).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_schema_cascade_empty_catalog_still_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/016.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            DropSchema::test("emptyschema").with_cascade(true).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert_eq!(findings.len(), 1, "Should fire even with empty catalog");
        assert!(findings[0].message.contains("emptyschema"));
    }

    #[test]
    fn test_drop_schema_cascade_prefix_no_false_match() {
        let before = CatalogBuilder::new()
            .table("myschema.orders", |t| {
                t.column("id", "bigint", false).pk(&["id"]);
            })
            .table("myschema_extra.users", |t| {
                t.column("id", "bigint", false).pk(&["id"]);
            })
            .build();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/016.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            DropSchema::test("myschema").with_cascade(true).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].message.contains("myschema.orders"),
            "Should list myschema.orders"
        );
        assert!(
            !findings[0].message.contains("myschema_extra"),
            "Should NOT match myschema_extra prefix"
        );
    }

    #[test]
    fn test_drop_schema_cascade_multiple_findings() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/016.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![
            located(DropSchema::test("schema_a").with_cascade(true).into()),
            located(DropSchema::test("schema_b").with_cascade(true).into()),
        ];

        let findings = rule_id().check(&stmts, &ctx);
        assert_eq!(findings.len(), 2);
        assert!(findings[0].message.contains("schema_a"));
        assert!(findings[1].message.contains("schema_b"));
    }

    #[test]
    fn test_drop_schema_no_cascade_no_finding() {
        let before = CatalogBuilder::new()
            .table("myschema.orders", |t| {
                t.column("id", "bigint", false).pk(&["id"]);
            })
            .build();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/016.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(DropSchema::test("myschema").into())];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "DROP SCHEMA without CASCADE should not fire"
        );
    }
}
