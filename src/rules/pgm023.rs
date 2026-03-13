//! PGM023 — Multiple ALTER TABLE statements on the same table
//!
//! When a migration contains multiple separate `ALTER TABLE` statements
//! targeting the same table with the same lock level, they should be combined
//! into a single statement. Each separate statement acquires and releases the
//! table lock independently, increasing lock contention time unnecessarily.

use std::collections::HashMap;

use crate::parser::ir::{AlterTableAction, IrNode, Located, SourceSpan};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str =
    "Multiple ALTER TABLE statements on the same table can be combined";

pub(super) const EXPLAIN: &str = "PGM023 — Multiple ALTER TABLE statements on the same table can be combined\n\
         \n\
         What it detects:\n\
         Multiple separate ALTER TABLE statements targeting the same table within\n\
         a single migration file, where all statements operate at the same lock\n\
         level.\n\
         \n\
         Why it matters:\n\
         Each ALTER TABLE statement acquires and releases the table lock\n\
         independently. Combining multiple actions into a single ALTER TABLE\n\
         statement acquires the lock only once, reducing the window during which\n\
         other sessions are blocked.\n\
         \n\
         Example (bad — two lock acquisitions):\n\
           ALTER TABLE authors ALTER COLUMN name SET NOT NULL;\n\
           ALTER TABLE authors ALTER COLUMN email SET NOT NULL;\n\
         \n\
         Fix (one lock acquisition):\n\
           ALTER TABLE authors\n\
             ALTER COLUMN name SET NOT NULL,\n\
             ALTER COLUMN email SET NOT NULL;\n\
         \n\
         Note: ALTER TABLE statements with different lock levels (e.g.,\n\
         ValidateConstraint vs SetNotNull) are tracked separately and will not\n\
         trigger this rule across different lock levels.";

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Minor;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    // Tracks the first ALTER TABLE seen per table key, per lock level:
    // key = (catalog_key, LockLevel), value = SourceSpan of first occurrence.
    let mut tracking: HashMap<(String, LockLevel), SourceSpan> = HashMap::new();

    for stmt in statements {
        match &stmt.node {
            IrNode::AlterTable(at) => {
                let key = at.name.catalog_key().to_string();

                // Only flag pre-existing tables — new tables have no lock contention risk.
                if !ctx.is_existing_table(&key) {
                    continue;
                }

                let lock = classify_lock_level(&at.actions);
                let map_key = (key.clone(), lock);

                if let Some(first_span) = tracking.get_mut(&map_key) {
                    // Second (or later) ALTER TABLE on this table with the same lock level.
                    let preceding_line = first_span.start_line;
                    findings.push(rule.make_finding(
                        format!(
                            "Table '{}' has multiple ALTER TABLE statements with the same lock \
                             level in this migration (preceding statement at line {}). \
                             Combine them into a single ALTER TABLE to reduce lock contention.",
                            at.name.display_name(),
                            preceding_line,
                        ),
                        ctx.file,
                        &stmt.span,
                    ));
                    // Update tracking to the current statement so the next firing points to
                    // the immediately preceding ALTER TABLE (keeps messages unique and
                    // gives more actionable "combine with line X" guidance).
                    *first_span = stmt.span.clone();
                } else {
                    // No existing chain for this (table, lock-level) pair — start one.
                    // The HashMap key includes LockLevel, so a chain at a different lock level
                    // for the same table remains unaffected.
                    tracking.insert(map_key, stmt.span.clone());
                }
            }
            node => {
                // Non-ALTER statement: if it references a table, break its chain.
                if let Some(key) = table_key_of(node) {
                    // Remove both lock-level chains for this table.
                    tracking.remove(&(key.clone(), LockLevel::AccessExclusive));
                    tracking.remove(&(key, LockLevel::ShareUpdateExclusive));
                }
                // Ignored and Unparseable nodes: no chain broken (table_key_of returns None).
            }
        }
    }

    findings
}

/// Lock level classification for a set of ALTER TABLE actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum LockLevel {
    /// All actions are `VALIDATE CONSTRAINT` — takes `SHARE UPDATE EXCLUSIVE`.
    ShareUpdateExclusive,
    /// Any other action — takes `ACCESS EXCLUSIVE`.
    AccessExclusive,
}

/// Classify the lock level required for a set of ALTER TABLE actions.
///
/// # Known simplification
/// `AttachPartition` actually takes `SHARE UPDATE EXCLUSIVE` in PostgreSQL, but
/// is currently classified as `AccessExclusive` here. Consecutive ATTACH PARTITION
/// statements on the same parent would benefit from combining but are not detected.
/// Deferred as rare enough not to warrant the added complexity in v1.
fn classify_lock_level(actions: &[AlterTableAction]) -> LockLevel {
    if !actions.is_empty()
        && actions
            .iter()
            .all(|a| matches!(a, AlterTableAction::ValidateConstraint { .. }))
    {
        LockLevel::ShareUpdateExclusive
    } else {
        LockLevel::AccessExclusive
    }
}

/// Extract the catalog key from non-ALTER IR nodes that reference a table.
///
/// Returns `None` for nodes that don't reference a table (or where the table
/// name is not available without a catalog lookup, like `DropIndex`).
fn table_key_of(node: &IrNode) -> Option<String> {
    match node {
        IrNode::CreateTable(ct) => Some(ct.name.catalog_key().to_string()),
        IrNode::CreateIndex(ci) => Some(ci.table_name.catalog_key().to_string()),
        IrNode::DropTable(dt) => Some(dt.name.catalog_key().to_string()),
        IrNode::TruncateTable(tt) => Some(tt.name.catalog_key().to_string()),
        IrNode::InsertInto(ii) => Some(ii.table_name.catalog_key().to_string()),
        IrNode::UpdateTable(ut) => Some(ut.table_name.catalog_key().to_string()),
        IrNode::DeleteFrom(df) => Some(df.table_name.catalog_key().to_string()),
        IrNode::Cluster(cl) => Some(cl.table.catalog_key().to_string()),
        IrNode::VacuumFull(vf) => vf.table.as_ref().map(|t| t.catalog_key().to_string()),
        IrNode::Reindex(_) => None,
        IrNode::RenameTable { name, .. } => Some(name.catalog_key().to_string()),
        IrNode::RenameColumn { table, .. } => Some(table.catalog_key().to_string()),
        // These don't have a table name to extract
        IrNode::AlterTable(_)
        | IrNode::DropIndex(_)
        | IrNode::DropSchema(_)
        | IrNode::AlterIndexAttachPartition { .. }
        | IrNode::Ignored { .. }
        | IrNode::Unparseable { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{lint_ctx, located, located_at};

    fn existing_catalog() -> Catalog {
        CatalogBuilder::new()
            .table("authors", |t| {
                t.column("id", "bigint", false)
                    .column("name", "text", true)
                    .column("email", "text", true)
                    .pk(&["id"]);
            })
            .table("orders", |t| {
                t.column("id", "bigint", false).pk(&["id"]);
            })
            .build()
    }

    fn alter_set_not_null(table: &str, col: &str) -> IrNode {
        IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::SetNotNull {
                column_name: col.to_string(),
            }],
        })
    }

    fn alter_validate(table: &str, constraint: &str) -> IrNode {
        IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::ValidateConstraint {
                constraint_name: constraint.to_string(),
            }],
        })
    }

    #[test]
    fn two_consecutive_alters_same_table_fires() {
        let before = existing_catalog();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![
            located_at(alter_set_not_null("authors", "name"), 1),
            located_at(alter_set_not_null("authors", "email"), 3),
        ];

        let findings = RuleId::Pgm023.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn three_consecutive_alters_fires_on_second_and_third() {
        let before = existing_catalog();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![
            located_at(alter_set_not_null("authors", "name"), 1),
            located_at(alter_set_not_null("authors", "email"), 3),
            located_at(
                IrNode::AlterTable(AlterTable {
                    name: QualifiedName::unqualified("authors"),
                    actions: vec![AlterTableAction::DropNotNull {
                        column_name: "email".to_string(),
                    }],
                }),
                5,
            ),
        ];

        let findings = RuleId::Pgm023.check(&stmts, &ctx);
        assert_eq!(findings.len(), 2);
    }

    #[test]
    fn two_alters_different_tables_no_finding() {
        let before = existing_catalog();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![
            located_at(alter_set_not_null("authors", "name"), 1),
            located_at(alter_set_not_null("orders", "id"), 3),
        ];

        let findings = RuleId::Pgm023.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn alters_separated_by_alter_on_different_table_still_fires() {
        // Chain on "authors" is NOT broken by an ALTER on "orders"
        let before = existing_catalog();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![
            located_at(alter_set_not_null("authors", "name"), 1),
            located_at(alter_set_not_null("orders", "id"), 3),
            located_at(alter_set_not_null("authors", "email"), 5),
        ];

        let findings = RuleId::Pgm023.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("authors"));
    }

    #[test]
    fn two_validate_constraint_alters_fires() {
        let before = existing_catalog();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![
            located_at(alter_validate("authors", "chk_name"), 1),
            located_at(alter_validate("authors", "chk_email"), 3),
        ];

        let findings = RuleId::Pgm023.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn schema_qualified_name_fires() {
        let before = CatalogBuilder::new()
            .table("myschema.authors", |t| {
                t.column("id", "bigint", false)
                    .column("name", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![
            located_at(
                IrNode::AlterTable(AlterTable {
                    name: QualifiedName::qualified("myschema", "authors"),
                    actions: vec![AlterTableAction::SetNotNull {
                        column_name: "name".to_string(),
                    }],
                }),
                1,
            ),
            located_at(
                IrNode::AlterTable(AlterTable {
                    name: QualifiedName::qualified("myschema", "authors"),
                    actions: vec![AlterTableAction::DropNotNull {
                        column_name: "name".to_string(),
                    }],
                }),
                3,
            ),
        ];

        let findings = RuleId::Pgm023.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("myschema.authors"));
    }

    #[test]
    fn single_alter_no_finding() {
        let before = existing_catalog();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(alter_set_not_null("authors", "name"))];

        let findings = RuleId::Pgm023.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn new_table_created_in_changeset_no_finding() {
        let before = Catalog::new();
        let after = existing_catalog();
        lint_ctx!(
            ctx,
            &before,
            &after,
            "migrations/001.sql",
            created: ["authors"]
        );

        let stmts = vec![
            located_at(alter_set_not_null("authors", "name"), 1),
            located_at(alter_set_not_null("authors", "email"), 3),
        ];

        let findings = RuleId::Pgm023.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn alters_broken_by_create_index_on_same_table() {
        let before = existing_catalog();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![
            located_at(alter_set_not_null("authors", "name"), 1),
            located_at(
                IrNode::CreateIndex(CreateIndex {
                    index_name: Some("idx_authors_name".to_string()),
                    table_name: QualifiedName::unqualified("authors"),
                    columns: vec![IndexColumn::Column("name".to_string())],
                    unique: false,
                    concurrent: true,
                    if_not_exists: false,
                    where_clause: None,
                    only: false,
                    access_method: "btree".to_string(),
                }),
                3,
            ),
            located_at(alter_set_not_null("authors", "email"), 5),
        ];

        let findings = RuleId::Pgm023.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn validate_then_set_not_null_different_lock_levels_no_finding() {
        let before = existing_catalog();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![
            located_at(alter_validate("authors", "chk_name"), 1),
            located_at(alter_set_not_null("authors", "name"), 3),
        ];

        let findings = RuleId::Pgm023.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn alters_broken_by_insert_into_same_table() {
        let before = existing_catalog();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![
            located_at(alter_set_not_null("authors", "name"), 1),
            located_at(
                IrNode::InsertInto(InsertInto {
                    table_name: QualifiedName::unqualified("authors"),
                }),
                3,
            ),
            located_at(alter_set_not_null("authors", "email"), 5),
        ];

        let findings = RuleId::Pgm023.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
