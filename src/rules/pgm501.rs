#![doc = include_str!("docs/pgm501.md")]

use crate::catalog::{Catalog, IndexState, TableState};
use crate::names::RelationName;
use crate::parser::ir::{AlterTableAction, IrNode, Located, SourceSpan, TableConstraint};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "Foreign key without covering index on referencing columns";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm501.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Major;

/// The closing sentence of every finding: what an uncovered foreign key costs.
const SEQUENTIAL_SCAN_COST: &str = "Queries joining or filtering on these columns, and \
     referential integrity checks, cause sequential scans on the referencing table.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let added_fks = fks_added_in(statements);
    let mut findings = added_fks_without_coverage(&rule, &added_fks, ctx);
    findings.extend(fks_left_without_coverage_by_drops(
        &rule, statements, &added_fks, ctx,
    ));
    findings
}

/// Every foreign key `statements` add, in `CREATE TABLE` or
/// `ALTER TABLE ... ADD CONSTRAINT`. The parser promotes a column's inline
/// `REFERENCES` to a table constraint, so those are included.
fn fks_added_in(statements: &[Located<IrNode>]) -> Vec<FkInfo> {
    let mut fks = Vec::new();
    for stmt in statements {
        match &stmt.node {
            IrNode::CreateTable(ct) => {
                for constraint in &ct.constraints {
                    if let TableConstraint::ForeignKey { columns, .. } = constraint {
                        fks.push(FkInfo {
                            table_name: ct.name.relation().clone(),
                            display_name: ct.name.display_name(),
                            columns: columns.clone(),
                            span: stmt.span.clone(),
                        });
                    }
                }
            }
            IrNode::AlterTable(at) => {
                for action in &at.actions {
                    if let AlterTableAction::AddConstraint {
                        constraint: TableConstraint::ForeignKey { columns, .. },
                        ..
                    } = action
                    {
                        fks.push(FkInfo {
                            table_name: at.name.relation().clone(),
                            display_name: at.name.display_name(),
                            columns: columns.clone(),
                            span: stmt.span.clone(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    fks
}

/// A finding for each foreign key in `added_fks` that has no covering index
/// once the whole unit is applied, so an index created later in the same
/// file still counts.
fn added_fks_without_coverage(
    rule: &impl Rule,
    added_fks: &[FkInfo],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for fk in added_fks {
        let has_index = match ctx.catalog_after.get_table(&fk.table_name) {
            Some(table) => match fk_coverage(ctx.catalog_after, table, &fk.columns) {
                Some(covered) => covered,
                None => continue,
            },
            None => false,
        };

        if !has_index {
            findings.push(rule.make_finding(
                format!(
                    "Foreign key on '{table}({cols})' has no covering index. \
                     {SEQUENTIAL_SCAN_COST}",
                    table = fk.display_name,
                    cols = fk.columns.join(", "),
                ),
                ctx.file,
                &fk.span,
            ));
        }
    }
    findings
}

/// A finding for each foreign key the unit leaves without a covering index by
/// dropping the indexes that covered it, while the key itself stays. The key
/// must exist before and after the unit, be covered before it and not after.
/// A key the unit adds is left to [`added_fks_without_coverage`]. When more
/// than one statement drops a covering index, the finding goes to the last,
/// which removes the coverage that was left.
fn fks_left_without_coverage_by_drops(
    rule: &impl Rule,
    statements: &[Located<IrNode>],
    added_fks: &[FkInfo],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings: Vec<((&RelationName, &str), Finding)> = Vec::new();
    for drop in index_drops(statements, ctx.catalog_before) {
        for (fk_table, fk_name, fk_columns) in fks_coverable_by(drop.table, ctx.catalog_after) {
            let added_in_unit = added_fks
                .iter()
                .any(|added| added.table_name == fk_table.name && added.columns == fk_columns);
            if added_in_unit {
                continue;
            }
            let Some(table_before) = ctx.catalog_before.get_table(&fk_table.name) else {
                continue;
            };
            let Some(fk_columns_before) = table_before
                .foreign_keys()
                .find_map(|(name, columns)| (name == fk_name).then_some(columns))
            else {
                continue;
            };

            let covering_names: Vec<&str> = drop
                .indexes
                .iter()
                .filter(|idx| idx.covers_fk_column(fk_columns_before))
                .map(|idx| idx.name.as_str())
                .collect();
            let coverage_lost = !covering_names.is_empty()
                && fk_coverage(ctx.catalog_before, table_before, fk_columns_before) == Some(true)
                && fk_coverage(ctx.catalog_after, fk_table, fk_columns) == Some(false);
            if !coverage_lost {
                continue;
            }

            let finding = rule.make_finding(
                format!(
                    "{dropping} leaves foreign key '{fk_name}' on '{table}({cols})' without a \
                     covering index. {SEQUENTIAL_SCAN_COST}",
                    dropping = drop.cause.describe(&covering_names),
                    table = fk_table.display_name,
                    cols = fk_columns.join(", "),
                ),
                ctx.file,
                drop.span,
            );
            let fk_key = (&fk_table.name, fk_name);
            findings.retain(|(key, _)| *key != fk_key);
            findings.push((fk_key, finding));
        }
    }
    findings.into_iter().map(|(_, finding)| finding).collect()
}

/// The foreign keys an index on `table` can cover, as `catalog` holds them:
/// `table`'s own, and those of its partition children, which a recursive
/// index on the parent covers.
fn fks_coverable_by<'a>(
    table: &TableState,
    catalog: &'a Catalog,
) -> Vec<(&'a TableState, &'a str, &'a [String])> {
    let partition_children = catalog.get_partition_children(&table.name);
    std::iter::once(&table.name)
        .chain(&partition_children)
        .filter_map(|key| catalog.get_table(key))
        .flat_map(|fk_table| {
            fk_table
                .foreign_keys()
                .map(move |(name, columns)| (fk_table, name, columns))
        })
        .collect()
}

/// Whether a foreign key over `fk_columns` on `table` has a covering index in
/// `catalog`, as [`TableState::has_indexed_fk_column`] judges one. A
/// partitioned table counts only its own recursive indexes. A partition child
/// without one of its own is covered by its parent's; `None` when that parent
/// is not in the catalog, so coverage is unknown.
fn fk_coverage(catalog: &Catalog, table: &TableState, fk_columns: &[String]) -> Option<bool> {
    if table.has_indexed_fk_column(fk_columns) {
        return Some(true);
    }
    match &table.parent_table {
        Some(parent) if !table.is_partitioned => catalog
            .get_table(parent)
            .map(|parent| parent.has_indexed_fk_column(fk_columns)),
        _ => Some(false),
    }
}

/// The statement that drops a set of indexes, as a finding describes it.
enum DropCause<'a> {
    /// `DROP INDEX`.
    DropIndex,
    /// `ALTER TABLE ... DROP CONSTRAINT` of a PRIMARY KEY, UNIQUE or EXCLUDE
    /// constraint, which drops the index of the same name.
    DropConstraint(&'a str),
    /// `ALTER TABLE ... DROP COLUMN`, which drops every index on the column.
    DropColumn(&'a str),
}

impl DropCause<'_> {
    /// The start of a finding's message: the statement, and which of the
    /// indexes it drops, `index_names`, covered the foreign key.
    fn describe(&self, index_names: &[&str]) -> String {
        let quoted = index_names
            .iter()
            .map(|name| format!("'{name}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let noun = if index_names.len() == 1 {
            "index"
        } else {
            "indexes"
        };
        match self {
            DropCause::DropIndex => format!("Dropping {noun} {quoted}"),
            DropCause::DropConstraint(constraint) => {
                format!("Dropping constraint '{constraint}' drops its index, which")
            }
            DropCause::DropColumn(column) => {
                format!("Dropping column '{column}' drops {noun} {quoted}, which")
            }
        }
    }
}

/// Indexes one statement drops from one table, as the catalog before the
/// unit records them.
struct IndexDrop<'a> {
    cause: DropCause<'a>,
    table: &'a TableState,
    indexes: Vec<&'a IndexState>,
    span: &'a SourceSpan,
}

/// Every set of indexes a statement in `statements` drops, in statement
/// order, looked up in `catalog_before`: `DROP INDEX`, `DROP CONSTRAINT` of an
/// index-backed constraint, and `DROP COLUMN`. An index the unit itself
/// creates is not in `catalog_before`, so dropping it again drops nothing
/// here. Neither does `DROP INDEX` of a constraint's index, which PostgreSQL
/// rejects (`cannot drop index ... because constraint ... requires it`).
fn index_drops<'a>(
    statements: &'a [Located<IrNode>],
    catalog_before: &'a Catalog,
) -> Vec<IndexDrop<'a>> {
    let mut drops = Vec::new();
    for stmt in statements {
        match &stmt.node {
            IrNode::DropIndex(di) => {
                let index = di.index_name.relation();
                let Some(table) = catalog_before
                    .table_for_index(index)
                    .and_then(|table| catalog_before.get_table(table))
                else {
                    continue;
                };
                let backs_a_constraint = table
                    .constraints
                    .iter()
                    .any(|c| c.is_index_backed() && c.name() == index.name);
                if backs_a_constraint {
                    continue;
                }
                drops.push(IndexDrop {
                    cause: DropCause::DropIndex,
                    table,
                    indexes: table
                        .indexes
                        .iter()
                        .filter(|idx| idx.name == index.name)
                        .collect(),
                    span: &stmt.span,
                });
            }
            IrNode::AlterTable(at) => {
                let Some(table) = catalog_before.get_table(at.name.relation()) else {
                    continue;
                };
                for action in &at.actions {
                    let (cause, indexes) = match action {
                        AlterTableAction::DropConstraint { constraint_name } => {
                            let backs_an_index = table
                                .constraints
                                .iter()
                                .any(|c| c.is_index_backed() && c.name() == constraint_name);
                            if !backs_an_index {
                                continue;
                            }
                            (
                                DropCause::DropConstraint(constraint_name),
                                table
                                    .indexes
                                    .iter()
                                    .filter(|idx| idx.name == *constraint_name)
                                    .collect(),
                            )
                        }
                        AlterTableAction::DropColumn { name } => (
                            DropCause::DropColumn(name),
                            table.indexes_involving_column(name),
                        ),
                        _ => continue,
                    };
                    drops.push(IndexDrop {
                        cause,
                        table,
                        indexes,
                        span: &stmt.span,
                    });
                }
            }
            _ => {}
        }
    }
    drops
}

/// A foreign key added in the current migration unit, with enough context
/// to report a finding.
struct FkInfo {
    table_name: RelationName,
    display_name: String,
    columns: Vec<String>,
    span: SourceSpan,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::parser::pg_query::parse_sql;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::*;
    use rstest::rstest;

    /// The findings for linting `change` on top of `history`, each replayed
    /// as the pipeline replays one file, with `public` as the default schema.
    fn findings_for_change(history: &str, change: &str) -> Vec<Finding> {
        let before = catalog_from_sql(history);
        let after = catalog_from_sql(&format!("{history}\n{change}"));
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");
        RuleId::Pgm501.check(&normalized(parse_sql(change)), &ctx)
    }

    fn messages_for_change(history: &str, change: &str) -> Vec<String> {
        findings_for_change(history, change)
            .into_iter()
            .map(|finding| finding.message)
            .collect()
    }

    const ORDERS_WITH_COVERED_FK: &str = "
        CREATE TABLE customers (id int PRIMARY KEY);
        CREATE TABLE orders (
            id int PRIMARY KEY,
            customer_id int,
            CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES customers (id)
        );
        CREATE INDEX idx_orders_customer_id ON orders (customer_id);";

    const SCAN_COST: &str = "Queries joining or filtering on these columns, and referential \
                             integrity checks, cause sequential scans on the referencing table.";

    #[test]
    fn test_fk_no_index_fires() {
        let before = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("child", |t| {
                t.column("id", "integer", false)
                    .column("pid", "integer", false)
                    .pk(&["id"]);
            })
            .build();
        // After: child has FK but no index
        let after = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("child", |t| {
                t.column("id", "integer", false)
                    .column("pid", "integer", false)
                    .pk(&["id"])
                    .fk("fk_parent", &["pid"], "parent", &["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("child"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_parent".to_string()),
                    columns: vec!["pid".to_string()],
                    ref_table: QualifiedName::unqualified("parent"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_fk_no_index_message_leads_with_query_cost_and_narrows_update_clause() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("child", |t| {
                t.column("pid", "integer", false)
                    .fk("fk_parent", &["pid"], "parent", &["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("child"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_parent".to_string()),
                    columns: vec!["pid".to_string()],
                    ref_table: QualifiedName::unqualified("parent"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        let message = &findings[0].message;
        assert!(
            message.contains("joining or filtering"),
            "message should name query joins/filters as a seq-scan cause, \
             the more frequent one, not just constraint enforcement: {message:?}"
        );
        assert!(
            !message.contains("deletes/updates on the referenced table"),
            "message must not imply any UPDATE on the referenced table triggers \
             the check — only deletes or updates to the referenced key do: {message:?}"
        );
    }

    #[test]
    fn test_fk_with_index_no_finding() {
        let before = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("child", |t| {
                t.column("id", "integer", false)
                    .column("pid", "integer", false)
                    .pk(&["id"]);
            })
            .build();
        // After: child has FK AND index
        let after = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("child", |t| {
                t.column("id", "integer", false)
                    .column("pid", "integer", false)
                    .pk(&["id"])
                    .fk("fk_parent", &["pid"], "parent", &["id"])
                    .index("idx_child_pid", &["pid"], false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("child"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_parent".to_string()),
                    columns: vec!["pid".to_string()],
                    ref_table: QualifiedName::unqualified("parent"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_fk_reversed_index_order_no_finding() {
        let before = Catalog::new();
        // After: child has composite FK (a, b) but index is (b, a)
        let after = CatalogBuilder::new()
            .table("child", |t| {
                t.column("a", "integer", false)
                    .column("b", "integer", false)
                    .fk("fk_composite", &["a", "b"], "parent", &["x", "y"])
                    .index("idx_wrong_order", &["b", "a"], false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("child"))
                .with_columns(vec![
                    ColumnDef::test("a", "integer").with_nullable(false),
                    ColumnDef::test("b", "integer").with_nullable(false),
                ])
                .with_constraints(vec![TableConstraint::ForeignKey {
                    name: Some("fk_composite".to_string()),
                    columns: vec!["a".to_string(), "b".to_string()],
                    ref_table: QualifiedName::unqualified("parent"),
                    ref_columns: vec!["x".to_string(), "y".to_string()],
                    not_valid: false,
                }]),
        ))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "Reversed-order index (b, a) still fully covers FK (a, b) — column order doesn't matter for an equality-only lookup"
        );
    }

    #[test]
    fn test_fk_prefix_match_no_finding() {
        let before = Catalog::new();
        // After: FK (a, b) with index (a, b, c) — prefix covers it
        let after = CatalogBuilder::new()
            .table("child", |t| {
                t.column("a", "integer", false)
                    .column("b", "integer", false)
                    .column("c", "integer", false)
                    .fk("fk_composite", &["a", "b"], "parent", &["x", "y"])
                    .index("idx_abc", &["a", "b", "c"], false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("child"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_composite".to_string()),
                    columns: vec!["a".to_string(), "b".to_string()],
                    ref_table: QualifiedName::unqualified("parent"),
                    ref_columns: vec!["x".to_string(), "y".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_fk_on_partitioned_table_no_index_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent_ref", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "parent_ref", &["id"])
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("parent_ref"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "FK on partitioned table without covering index should fire"
        );
    }

    #[test]
    fn test_fk_on_partition_child_suppressed() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent_ref", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "parent_ref", &["id"])
                    .partition_of("orders_parent");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("parent_ref"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_fk_on_partitioned_table_via_create_table_fires() {
        // FK defined inline in CREATE TABLE on a partitioned table, no index.
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("ref_table", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "ref_table", &["id"])
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders"))
                .with_columns(vec![
                    ColumnDef::test("id", "integer").with_nullable(false),
                    ColumnDef::test("ref_id", "integer").with_nullable(false),
                ])
                .with_constraints(vec![TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("ref_table"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                }])
                .with_partition_by(
                    crate::parser::ir::PartitionStrategy::Range,
                    vec!["id".to_string()],
                ),
        ))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "FK via CREATE TABLE on partitioned table without index should fire"
        );
    }

    #[test]
    fn test_fk_with_partial_index_fires() {
        let before = Catalog::new();
        // After: child has FK and a partial index covering the FK columns,
        // but partial indexes don't count for FK coverage.
        let after = CatalogBuilder::new()
            .table("child", |t| {
                t.column("pid", "integer", false)
                    .fk("fk_parent", &["pid"], "parent", &["id"])
                    .partial_index("idx_pid_active", &["pid"], false, "active = true");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("child"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_parent".to_string()),
                    columns: vec!["pid".to_string()],
                    ref_table: QualifiedName::unqualified("parent"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Partial index should not satisfy FK coverage"
        );
    }

    #[test]
    fn test_fk_with_expression_index_fires() {
        let before = Catalog::new();
        // After: child has FK on (pid) but index is on (lower(pid::text)) — expression
        let after = CatalogBuilder::new()
            .table("child", |t| {
                t.column("pid", "integer", false)
                    .fk("fk_parent", &["pid"], "parent", &["id"])
                    .expression_index("idx_pid_expr", &["expr:lower(pid::text)"], false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("child"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_parent".to_string()),
                    columns: vec!["pid".to_string()],
                    ref_table: QualifiedName::unqualified("parent"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Expression index should not satisfy FK coverage"
        );
    }

    #[test]
    fn test_fk_on_partitioned_table_with_regular_index_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent_ref", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "parent_ref", &["id"])
                    .index("idx_orders_ref_id", &["ref_id"], false)
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("parent_ref"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "Recursive index on partitioned table should satisfy FK coverage"
        );
    }

    #[test]
    fn test_fk_partial_coverage_on_partitioned_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("ref_table", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("postings", |t| {
                t.column("transaction_id", "integer", false)
                    .column("partition_key", "integer", false)
                    .fk(
                        "fk_txn",
                        &["transaction_id", "partition_key"],
                        "ref_table",
                        &["id"],
                    )
                    .index("idx_txn", &["transaction_id"], false)
                    .partitioned_by(
                        crate::parser::ir::PartitionStrategy::Range,
                        &["partition_key"],
                    );
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("postings"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_txn".to_string()),
                    columns: vec!["transaction_id".to_string(), "partition_key".to_string()],
                    ref_table: QualifiedName::unqualified("ref_table"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "Index on only one FK column still avoids a seq scan (Index Scan + Filter), \
             so PGM501 should not fire — this is the real production false positive"
        );
    }

    #[test]
    fn test_fk_on_partitioned_table_with_only_index_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent_ref", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "parent_ref", &["id"])
                    .only_index("idx_orders_ref_id", &["ref_id"], false)
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("parent_ref"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "ON ONLY index should NOT satisfy FK coverage"
        );
    }

    #[test]
    fn test_fk_on_partitioned_table_after_attach_no_finding() {
        // ON ONLY index was created, then ALTER INDEX ATTACH flipped only to false.
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent_ref", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "parent_ref", &["id"])
                    .index("idx_orders_ref_id", &["ref_id"], false) // only=false after attach
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("parent_ref"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "After ATTACH, index should satisfy FK coverage"
        );
    }

    #[test]
    fn test_fk_on_partition_child_with_own_index_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent_ref", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "parent_ref", &["id"])
                    .index("idx_child_ref_id", &["ref_id"], false)
                    .partition_of("orders_parent");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("parent_ref"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(findings.is_empty(), "Child with own index should not fire");
    }

    #[test]
    fn test_fk_with_gin_index_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("customer_id", "integer", false)
                    .pk(&["id"]);
            })
            .table("customers", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("customer_id", "integer", false)
                    .pk(&["id"])
                    .fk("fk_cust", &["customer_id"], "customers", &["id"])
                    .index_with_method("idx_gin_cust", &["customer_id"], false, "gin");
            })
            .table("customers", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_cust".to_string()),
                    columns: vec!["customer_id".to_string()],
                    ref_table: QualifiedName::unqualified("customers"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "GIN index should NOT satisfy FK coverage — only btree indexes can"
        );
    }

    #[test]
    fn test_fk_with_gist_index_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("customer_id", "integer", false)
                    .pk(&["id"]);
            })
            .table("customers", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("customer_id", "integer", false)
                    .pk(&["id"])
                    .fk("fk_cust", &["customer_id"], "customers", &["id"])
                    .index_with_method("idx_gist_cust", &["customer_id"], false, "gist");
            })
            .table("customers", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_cust".to_string()),
                    columns: vec!["customer_id".to_string()],
                    ref_table: QualifiedName::unqualified("customers"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "GiST index should NOT satisfy FK coverage — only btree indexes can"
        );
    }

    #[test]
    fn test_fk_on_partition_child_delegates_to_parent() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent_ref", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders_parent", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .index("idx_parent_ref_id", &["ref_id"], false)
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .table("orders_child", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "parent_ref", &["id"])
                    .partition_of("orders_parent");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders_child"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("parent_ref"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "Child should delegate to parent's recursive index"
        );
    }

    #[test]
    fn test_fk_on_partition_child_parent_only_index_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent_ref", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders_parent", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .only_index("idx_parent_ref_id", &["ref_id"], false)
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .table("orders_child", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "parent_ref", &["id"])
                    .partition_of("orders_parent");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders_child"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("parent_ref"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Parent's ON ONLY index should NOT satisfy child FK coverage"
        );
    }

    #[rstest]
    #[case::plain("DROP INDEX idx_orders_customer_id;")]
    #[case::concurrently("DROP INDEX CONCURRENTLY idx_orders_customer_id;")]
    #[case::if_exists("DROP INDEX IF EXISTS idx_orders_customer_id;")]
    fn dropping_the_only_covering_index_fires(#[case] change: &str) {
        let messages = messages_for_change(ORDERS_WITH_COVERED_FK, change);
        assert_eq!(
            messages,
            [format!(
                "Dropping index 'idx_orders_customer_id' leaves foreign key \
                 'fk_orders_customer' on 'orders(customer_id)' without a covering index. \
                 {SCAN_COST}"
            )]
        );
    }

    #[test]
    fn dropping_the_only_covering_index_snapshot() {
        let findings =
            findings_for_change(ORDERS_WITH_COVERED_FK, "DROP INDEX idx_orders_customer_id;");
        insta::assert_yaml_snapshot!(findings);
    }

    #[rstest]
    #[case::schema_a("schema_a", "schema_a.children")]
    #[case::schema_b("schema_b", "schema_b.children")]
    fn dropping_a_schemas_idx_fk_fires_for_that_schema_only(
        #[case] schema: &str,
        #[case] table: &str,
    ) {
        let history = "
            CREATE TABLE schema_a.parents (id int PRIMARY KEY);
            CREATE TABLE schema_a.children (
                id int PRIMARY KEY,
                parent_id int CONSTRAINT fk_parent REFERENCES schema_a.parents (id)
            );
            CREATE INDEX idx_fk ON schema_a.children (parent_id);
            CREATE TABLE schema_b.parents (id int PRIMARY KEY);
            CREATE TABLE schema_b.children (
                id int PRIMARY KEY,
                parent_id int CONSTRAINT fk_parent REFERENCES schema_b.parents (id)
            );
            CREATE INDEX idx_fk ON schema_b.children (parent_id);";

        let messages = messages_for_change(history, &format!("DROP INDEX {schema}.idx_fk;"));
        assert_eq!(
            messages,
            [format!(
                "Dropping index 'idx_fk' leaves foreign key 'fk_parent' on \
                 '{table}(parent_id)' without a covering index. {SCAN_COST}"
            )]
        );
    }

    #[test]
    fn unqualified_drop_index_fires_for_the_default_schema_only() {
        let history = "
            CREATE TABLE parents (id int PRIMARY KEY);
            CREATE TABLE children (
                id int PRIMARY KEY,
                parent_id int CONSTRAINT fk_parent REFERENCES parents (id)
            );
            CREATE INDEX idx_fk ON children (parent_id);
            CREATE TABLE schema_b.children (
                id int PRIMARY KEY,
                parent_id int CONSTRAINT fk_parent REFERENCES parents (id)
            );
            CREATE INDEX idx_fk ON schema_b.children (parent_id);";

        let messages = messages_for_change(history, "DROP INDEX idx_fk;");
        assert_eq!(
            messages,
            [format!(
                "Dropping index 'idx_fk' leaves foreign key 'fk_parent' on \
                 'children(parent_id)' without a covering index. {SCAN_COST}"
            )]
        );
    }

    #[rstest]
    #[case::unique(
        "CONSTRAINT uq_orders_customer UNIQUE (customer_id)",
        "uq_orders_customer"
    )]
    #[case::primary_key("CONSTRAINT orders_pkey PRIMARY KEY (customer_id, id)", "orders_pkey")]
    fn dropping_a_constraint_whose_index_covered_the_fk_fires(
        #[case] index_backed_constraint: &str,
        #[case] constraint_name: &str,
    ) {
        let history = format!(
            "CREATE TABLE customers (id int PRIMARY KEY);
             CREATE TABLE orders (
                 id int,
                 customer_id int,
                 {index_backed_constraint},
                 CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES customers (id)
             );"
        );

        let messages = messages_for_change(
            &history,
            &format!("ALTER TABLE orders DROP CONSTRAINT {constraint_name};"),
        );
        assert_eq!(
            messages,
            [format!(
                "Dropping constraint '{constraint_name}' drops its index, which leaves foreign \
                 key 'fk_orders_customer' on 'orders(customer_id)' without a covering index. \
                 {SCAN_COST}"
            )]
        );
    }

    #[test]
    fn dropping_a_column_of_the_only_covering_index_fires() {
        let history = "
            CREATE TABLE customers (id int PRIMARY KEY);
            CREATE TABLE orders (
                id int PRIMARY KEY,
                customer_id int,
                legacy_code text,
                CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES customers (id)
            );
            CREATE INDEX idx_orders_customer_legacy ON orders (customer_id, legacy_code);";

        let messages =
            messages_for_change(history, "ALTER TABLE orders DROP COLUMN legacy_code;");
        assert_eq!(
            messages,
            [format!(
                "Dropping column 'legacy_code' drops index 'idx_orders_customer_legacy', which \
                 leaves foreign key 'fk_orders_customer' on 'orders(customer_id)' without a \
                 covering index. {SCAN_COST}"
            )]
        );
    }

    #[test]
    fn dropping_a_column_that_takes_two_covering_indexes_names_both() {
        let history = "
            CREATE TABLE customers (id int PRIMARY KEY);
            CREATE TABLE orders (
                id int PRIMARY KEY,
                customer_id int,
                legacy_code text,
                CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES customers (id)
            );
            CREATE INDEX idx_orders_customer_legacy ON orders (customer_id, legacy_code);
            CREATE INDEX idx_orders_legacy_customer ON orders (legacy_code, customer_id);";

        let messages =
            messages_for_change(history, "ALTER TABLE orders DROP COLUMN legacy_code;");
        assert_eq!(
            messages,
            [format!(
                "Dropping column 'legacy_code' drops indexes 'idx_orders_customer_legacy', \
                 'idx_orders_legacy_customer', which leaves foreign key 'fk_orders_customer' on \
                 'orders(customer_id)' without a covering index. {SCAN_COST}"
            )]
        );
    }

    #[rstest]
    #[case::second_index_on_the_fk_column(
        "CREATE INDEX idx_orders_customer_created ON orders (customer_id, id);"
    )]
    #[case::index_containing_the_fk_column_anywhere("CREATE INDEX idx_orders_id_customer ON orders (id, customer_id);")]
    #[case::hash_index("CREATE INDEX idx_orders_customer_hash ON orders USING hash (customer_id);")]
    fn another_index_still_covering_the_fk_is_silent(#[case] second_index: &str) {
        let history = format!("{ORDERS_WITH_COVERED_FK}\n{second_index}");
        let messages = messages_for_change(&history, "DROP INDEX idx_orders_customer_id;");
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[rstest]
    #[case::partial_index("CREATE INDEX idx_orders_customer_open ON orders (customer_id) WHERE id > 0;")]
    #[case::gin_index("CREATE INDEX idx_orders_customer_gin ON orders USING gin (customer_id);")]
    fn an_index_that_does_not_count_as_coverage_does_not_keep_it(#[case] second_index: &str) {
        let history = format!("{ORDERS_WITH_COVERED_FK}\n{second_index}");
        let messages = messages_for_change(&history, "DROP INDEX idx_orders_customer_id;");
        assert_eq!(messages.len(), 1, "got {messages:?}");
    }

    #[rstest]
    #[case::drop_constraint_first(
        "ALTER TABLE orders DROP CONSTRAINT fk_orders_customer;
         DROP INDEX idx_orders_customer_id;"
    )]
    #[case::drop_constraint_after(
        "DROP INDEX idx_orders_customer_id;
         ALTER TABLE orders DROP CONSTRAINT fk_orders_customer;"
    )]
    #[case::drop_the_fk_column("ALTER TABLE orders DROP COLUMN customer_id;")]
    #[case::drop_the_table(
        "DROP INDEX idx_orders_customer_id;
         DROP TABLE orders;"
    )]
    fn fk_dropped_in_the_same_unit_is_silent(#[case] change: &str) {
        let messages = messages_for_change(ORDERS_WITH_COVERED_FK, change);
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[rstest]
    #[case::new_fk_on_the_same_column(
        "ALTER TABLE orders ADD CONSTRAINT fk_orders_customer_again
             FOREIGN KEY (customer_id) REFERENCES customers (id);
         DROP INDEX idx_orders_customer_id;"
    )]
    #[case::fk_dropped_and_added_back_under_its_name(
        "ALTER TABLE orders DROP CONSTRAINT fk_orders_customer;
         ALTER TABLE orders ADD CONSTRAINT fk_orders_customer
             FOREIGN KEY (customer_id) REFERENCES customers (id);
         DROP INDEX idx_orders_customer_id;"
    )]
    fn fk_added_in_the_same_unit_is_reported_once_by_the_add_check(#[case] change: &str) {
        let messages = messages_for_change(ORDERS_WITH_COVERED_FK, change);
        assert_eq!(
            messages,
            [format!(
                "Foreign key on 'orders(customer_id)' has no covering index. {SCAN_COST}"
            )]
        );
    }

    #[test]
    fn composite_fk_keeps_coverage_from_an_index_on_one_of_its_columns() {
        let history = "
            CREATE TABLE parents (a int, b int, PRIMARY KEY (a, b));
            CREATE TABLE children (
                id int PRIMARY KEY,
                a int,
                b int,
                CONSTRAINT fk_parent FOREIGN KEY (a, b) REFERENCES parents (a, b)
            );
            CREATE INDEX idx_children_a_b ON children (a, b);
            CREATE INDEX idx_children_b ON children (b);";

        let messages = messages_for_change(history, "DROP INDEX idx_children_a_b;");
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[test]
    fn composite_fk_losing_its_only_covering_index_fires_with_all_its_columns() {
        let history = "
            CREATE TABLE parents (a int, b int, PRIMARY KEY (a, b));
            CREATE TABLE children (
                id int PRIMARY KEY,
                a int,
                b int,
                CONSTRAINT fk_parent FOREIGN KEY (a, b) REFERENCES parents (a, b)
            );
            CREATE INDEX idx_children_b_id ON children (b, id);";

        let messages = messages_for_change(history, "DROP INDEX idx_children_b_id;");
        assert_eq!(
            messages,
            [format!(
                "Dropping index 'idx_children_b_id' leaves foreign key 'fk_parent' on \
                 'children(a, b)' without a covering index. {SCAN_COST}"
            )]
        );
    }

    #[test]
    fn dropping_two_covering_indexes_reports_once_on_the_last_drop() {
        let history = format!(
            "{ORDERS_WITH_COVERED_FK}
             CREATE INDEX idx_orders_customer_id_id ON orders (customer_id, id);"
        );

        let findings = findings_for_change(
            &history,
            "DROP INDEX idx_orders_customer_id;\nDROP INDEX idx_orders_customer_id_id;",
        );
        let reported: Vec<(usize, &str)> = findings
            .iter()
            .map(|finding| (finding.start_line, finding.message.as_str()))
            .collect();
        assert_eq!(
            reported,
            [(
                2,
                format!(
                    "Dropping index 'idx_orders_customer_id_id' leaves foreign key \
                     'fk_orders_customer' on 'orders(customer_id)' without a covering index. \
                     {SCAN_COST}"
                )
                .as_str()
            )]
        );
    }

    #[test]
    fn drop_index_of_a_constraints_index_is_silent_as_postgres_rejects_it() {
        let history = "
            CREATE TABLE customers (id int PRIMARY KEY);
            CREATE TABLE orders (
                id int PRIMARY KEY,
                customer_id int,
                CONSTRAINT uq_orders_customer UNIQUE (customer_id),
                CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES customers (id)
            );";

        let messages = messages_for_change(history, "DROP INDEX uq_orders_customer;");
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[rstest]
    #[case::index_on_another_column("CREATE INDEX idx_orders_id ON orders (id);", "idx_orders_id")]
    #[case::fk_never_covered("CREATE INDEX idx_customers_id ON customers (id);", "idx_customers_id")]
    fn dropping_an_index_that_never_covered_the_fk_is_silent(
        #[case] unrelated_index: &str,
        #[case] index_name: &str,
    ) {
        let history = format!(
            "CREATE TABLE customers (id int PRIMARY KEY);
             CREATE TABLE orders (
                 id int PRIMARY KEY,
                 customer_id int,
                 CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES customers (id)
             );
             {unrelated_index}"
        );
        let messages = messages_for_change(&history, &format!("DROP INDEX {index_name};"));
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[test]
    fn dropping_a_partitioned_parents_index_fires_for_a_child_fk_it_covered() {
        let history = "
            CREATE TABLE refs (id int PRIMARY KEY);
            CREATE TABLE orders (id int, ref_id int) PARTITION BY RANGE (id);
            CREATE INDEX idx_orders_ref_id ON orders (ref_id);
            CREATE TABLE orders_2024 PARTITION OF orders FOR VALUES FROM (0) TO (100);
            ALTER TABLE orders_2024 ADD CONSTRAINT fk_ref FOREIGN KEY (ref_id) REFERENCES refs (id);";

        let messages = messages_for_change(history, "DROP INDEX idx_orders_ref_id;");
        assert_eq!(
            messages,
            [format!(
                "Dropping index 'idx_orders_ref_id' leaves foreign key 'fk_ref' on \
                 'orders_2024(ref_id)' without a covering index. {SCAN_COST}"
            )]
        );
    }

    #[test]
    fn dropping_a_partition_childs_own_index_is_silent_while_the_parent_covers_it() {
        let history = "
            CREATE TABLE refs (id int PRIMARY KEY);
            CREATE TABLE orders (id int, ref_id int) PARTITION BY RANGE (id);
            CREATE INDEX idx_orders_ref_id ON orders (ref_id);
            CREATE TABLE orders_2024 PARTITION OF orders FOR VALUES FROM (0) TO (100);
            CREATE INDEX idx_orders_2024_ref_id ON orders_2024 (ref_id);
            ALTER TABLE orders_2024 ADD CONSTRAINT fk_ref FOREIGN KEY (ref_id) REFERENCES refs (id);";

        let messages = messages_for_change(history, "DROP INDEX idx_orders_2024_ref_id;");
        assert!(messages.is_empty(), "got {messages:?}");
    }
}
