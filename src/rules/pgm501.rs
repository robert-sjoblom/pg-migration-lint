#![doc = include_str!("docs/pgm501.md")]

use crate::catalog::replay::map_with_state_before;
use crate::catalog::{Catalog, IndexState, TableState};
use crate::names::RelationName;
use crate::parser::ir::{AlterTableAction, IrNode, Located, SourceSpan, TableConstraint};
use crate::rules::rejected_statements::RejectedStatements;
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
/// dropping the indexes that covered it, while the key itself stays
/// ([`fks_losing_coverage`]). The finding goes to the statement that ends the
/// key's coverage for the last time in the unit: the key is covered just
/// before the statement and not just after it. Each statement is judged
/// against `catalog_before` with the unit's earlier statements replayed on
/// top, so an index renamed or created earlier in the unit is found under the
/// name the statement drops it by. There is no finding when that statement
/// drops no covering index, as with `DETACH PARTITION`, which PGM501 does not
/// judge, or, while PGM027 is enabled, when PostgreSQL rejects it:
/// `DROP INDEX covering_idx, constraint_idx` fails as a whole, though replay
/// drops `covering_idx`, and PGM027 reports it. While PGM027 is disabled, the
/// drop counts as replay applies it. A unit without a statement that can drop
/// an index, or that loses no key's coverage, is not replayed.
fn fks_left_without_coverage_by_drops(
    rule: &impl Rule,
    statements: &[Located<IrNode>],
    added_fks: &[FkInfo],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    if !statements.iter().any(|stmt| may_drop_an_index(&stmt.node)) {
        return Vec::new();
    }
    let losing_coverage = fks_losing_coverage(added_fks, ctx);
    if losing_coverage.is_empty() {
        return Vec::new();
    }
    let before_each_statement: Vec<Vec<FkBeforeStatement<'_>>> =
        map_with_state_before(ctx.catalog_before, statements, |state, stmt| {
            let drops = index_drops(state, stmt);
            losing_coverage
                .iter()
                .map(|fk| fk.before_statement(state, &drops))
                .collect()
        });

    let rejected = RejectedStatements::new(ctx, statements);
    let mut findings: Vec<(usize, Finding)> = Vec::new();
    for (position, fk) in losing_coverage.iter().enumerate() {
        let fk_before_each_statement: Vec<&FkBeforeStatement<'_>> = before_each_statement
            .iter()
            .filter_map(|fks| fks.get(position))
            .collect();
        let Some((stmt_index, drop)) = last_drop_ending_coverage(&fk_before_each_statement) else {
            continue;
        };
        let Some(stmt) = statements.get(stmt_index) else {
            continue;
        };
        if rejected.contains(&stmt.span) {
            continue;
        }
        let finding = rule.make_finding(
            format!(
                "{dropping} leaves foreign key '{fk_name}' on '{table}({cols})' without a \
                 covering index. {SEQUENTIAL_SCAN_COST}",
                dropping = drop.cause.describe(&drop.index_names),
                fk_name = fk.name,
                table = fk.table.display_name,
                cols = fk.columns.join(", "),
            ),
            ctx.file,
            &stmt.span,
        );
        findings.push((stmt_index, finding));
    }
    findings.sort_by_key(|(stmt_index, _)| *stmt_index);
    findings.into_iter().map(|(_, finding)| finding).collect()
}

/// The foreign keys whose coverage the unit loses: a key on a table of the
/// same name, with the same constraint name, in `catalog_before` and
/// `catalog_after`, that a usable index covers before the unit and none
/// after it ([`fk_coverage`]). A key the unit adds on the same table and
/// columns is left to [`added_fks_without_coverage`]. Coverage the unit adds
/// and takes away again, such as a temporary index for a backfill on a key
/// that had none, is not lost. Ordered by table, then as the table lists its
/// keys.
fn fks_losing_coverage<'a>(
    added_fks: &[FkInfo],
    ctx: &LintContext<'a>,
) -> Vec<FkLosingCoverage<'a>> {
    let mut losing_coverage: Vec<FkLosingCoverage<'a>> = ctx
        .catalog_after
        .tables()
        .flat_map(|table| {
            table
                .foreign_keys()
                .map(move |(name, columns)| FkLosingCoverage {
                    table,
                    name,
                    columns,
                })
        })
        .filter(|fk| {
            !added_fks
                .iter()
                .any(|added| added.table_name == fk.table.name && added.columns == fk.columns)
        })
        .filter(|fk| fk_coverage(ctx.catalog_after, fk.table, fk.columns) == Some(false))
        .filter(|fk| {
            fk.in_catalog(ctx.catalog_before)
                .is_some_and(|(table, columns)| {
                    fk_coverage(ctx.catalog_before, table, columns) == Some(true)
                })
        })
        .collect();
    losing_coverage.sort_by(|a, b| a.table.name.cmp(&b.table.name));
    losing_coverage
}

/// The statement that ends a foreign key's coverage for the last time in the
/// unit, by its index, and the covering indexes it drops. `before_each` holds
/// the key just before each statement, so the key just after a statement is
/// as the next one sees it, and after the last as `catalog_after` holds it:
/// not covered, as [`fks_losing_coverage`] requires. `None` when no
/// statement ends the key's coverage, or the last that does drops no index
/// covering it.
fn last_drop_ending_coverage<'f, 's>(
    before_each: &[&'f FkBeforeStatement<'s>],
) -> Option<(usize, &'f CoveringDrop<'s>)> {
    let covered_after_each = before_each
        .iter()
        .skip(1)
        .map(|fk| fk.covered)
        .chain([Some(false)]);
    let (stmt_index, (ending, _)) = before_each
        .iter()
        .zip(covered_after_each)
        .enumerate()
        .filter(|(_, (before, covered_after))| {
            before.covered == Some(true) && *covered_after == Some(false)
        })
        .last()?;
    Some((stmt_index, ending.covering_drop.as_ref()?))
}

/// True for a statement [`index_drops`] can find a drop in. In a unit
/// without one, no statement can end a key's coverage by dropping its index.
fn may_drop_an_index(node: &IrNode) -> bool {
    match node {
        IrNode::DropIndex(_) => true,
        IrNode::AlterTable(at) => at.actions.iter().any(|action| {
            matches!(
                action,
                AlterTableAction::DropConstraint { .. } | AlterTableAction::DropColumn { .. }
            )
        }),
        _ => false,
    }
}

/// Whether an index on `index_table` counts as coverage for a foreign key on
/// `fk_table`, as [`fk_coverage`] counts it: an index on the key's own table,
/// or on its parent when the table is a partition that is not partitioned
/// itself.
fn index_table_counts_for(index_table: &RelationName, fk_table: &TableState) -> bool {
    *index_table == fk_table.name
        || (!fk_table.is_partitioned && fk_table.parent_table.as_ref() == Some(index_table))
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
#[expect(clippy::enum_variant_names, reason = "we're okay with this")]
#[derive(Clone, Copy)]
enum DropCause<'s> {
    /// `DROP INDEX`.
    DropIndex,
    /// `ALTER TABLE ... DROP CONSTRAINT` of a PRIMARY KEY, UNIQUE or EXCLUDE
    /// constraint, which drops the index of the same name.
    DropConstraint(&'s str),
    /// `ALTER TABLE ... DROP COLUMN`, which drops every index on the column.
    DropColumn(&'s str),
}

impl DropCause<'_> {
    /// The start of a finding's message: the statement, and which of the
    /// indexes it drops, `index_names`, covered the foreign key.
    fn describe(&self, index_names: &[String]) -> String {
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

/// Indexes one statement drops from one table, as the catalog just before
/// the statement records them.
struct IndexDrop<'c, 's> {
    cause: DropCause<'s>,
    table: &'c RelationName,
    indexes: Vec<&'c IndexState>,
}

impl<'s> IndexDrop<'_, 's> {
    /// The indexes of this drop that cover a foreign key over `fk_columns` on
    /// `fk_table`, as the same catalog holds the key. `None` when none does,
    /// or the dropped indexes' table does not count for the key
    /// ([`index_table_counts_for`]).
    fn covering(&self, fk_table: &TableState, fk_columns: &[String]) -> Option<CoveringDrop<'s>> {
        if !index_table_counts_for(self.table, fk_table) {
            return None;
        }
        let index_names: Vec<String> = self
            .indexes
            .iter()
            .filter(|idx| idx.covers_fk_column(fk_columns))
            .map(|idx| idx.name.clone())
            .collect();
        (!index_names.is_empty()).then_some(CoveringDrop {
            cause: self.cause,
            index_names,
        })
    }
}

/// Every set of indexes `stmt` drops, looked up in `catalog`, the catalog
/// just before it: `DROP INDEX`, `DROP CONSTRAINT` of an index-backed
/// constraint, and `DROP COLUMN`, one per action of an `ALTER TABLE`, in
/// order. An index renamed or created earlier in the unit is in `catalog`
/// under the name the statement uses. `DROP INDEX` of a constraint's index
/// drops nothing here, as PostgreSQL rejects it (`cannot drop index ...
/// because constraint ... requires it`): replay keeps the index whether or
/// not PGM027 is enabled, and PGM027, while enabled, reports the statement.
fn index_drops<'c, 's>(catalog: &'c Catalog, stmt: &'s Located<IrNode>) -> Vec<IndexDrop<'c, 's>> {
    match &stmt.node {
        IrNode::DropIndex(di) => {
            let index = di.index_name.relation();
            let Some(table) = catalog
                .table_for_index(index)
                .and_then(|table| catalog.get_table(table))
            else {
                return Vec::new();
            };
            if table.constraint_backed_by(&index.name).is_some() {
                return Vec::new();
            }
            vec![IndexDrop {
                cause: DropCause::DropIndex,
                table: &table.name,
                indexes: table
                    .indexes
                    .iter()
                    .filter(|idx| idx.name == index.name)
                    .collect(),
            }]
        }
        IrNode::AlterTable(at) => {
            let Some(table) = catalog.get_table(at.name.relation()) else {
                return Vec::new();
            };
            at.actions
                .iter()
                .filter_map(|action| {
                    let (cause, indexes) = match action {
                        AlterTableAction::DropConstraint { constraint_name } => {
                            let backs_an_index = table
                                .constraints
                                .iter()
                                .any(|c| c.is_index_backed() && c.name() == constraint_name);
                            if !backs_an_index {
                                return None;
                            }
                            (
                                DropCause::DropConstraint(constraint_name.as_str()),
                                table
                                    .indexes
                                    .iter()
                                    .filter(|idx| idx.name == *constraint_name)
                                    .collect(),
                            )
                        }
                        AlterTableAction::DropColumn { name } => (
                            DropCause::DropColumn(name.as_str()),
                            table.indexes_involving_column(name),
                        ),
                        _ => return None,
                    };
                    Some(IndexDrop {
                        cause,
                        table: &table.name,
                        indexes,
                    })
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

/// A foreign key whose coverage the unit loses ([`fks_losing_coverage`]),
/// as `catalog_after` holds it.
struct FkLosingCoverage<'a> {
    table: &'a TableState,
    name: &'a str,
    columns: &'a [String],
}

impl FkLosingCoverage<'_> {
    /// The key's table and columns in `catalog`: the table of the same name,
    /// and its foreign key of the same name. `None` when `catalog` has no
    /// such table, or no such key on it.
    fn in_catalog<'c>(&self, catalog: &'c Catalog) -> Option<(&'c TableState, &'c [String])> {
        let table = catalog.get_table(&self.table.name)?;
        let columns = table
            .foreign_keys()
            .find_map(|(name, columns)| (name == self.name).then_some(columns))?;
        Some((table, columns))
    }

    /// The key as `state`, the catalog just before a statement, holds it,
    /// with the last of the statement's `drops` that takes indexes covering
    /// it.
    fn before_statement<'s>(
        &self,
        state: &Catalog,
        drops: &[IndexDrop<'_, 's>],
    ) -> FkBeforeStatement<'s> {
        let Some((table, columns)) = self.in_catalog(state) else {
            return FkBeforeStatement {
                covered: None,
                covering_drop: None,
            };
        };
        FkBeforeStatement {
            covered: fk_coverage(state, table, columns),
            covering_drop: drops
                .iter()
                .rev()
                .find_map(|drop| drop.covering(table, columns)),
        }
    }
}

/// A foreign key losing coverage, as the catalog holds it just before one
/// statement of the unit.
struct FkBeforeStatement<'s> {
    /// Whether a usable index covers the key ([`fk_coverage`]). `None` when
    /// the catalog has no such key just then, or cannot tell.
    covered: Option<bool>,
    /// The last drop of the statement that takes indexes covering the key.
    covering_drop: Option<CoveringDrop<'s>>,
}

/// The indexes covering a foreign key that one drop takes, by name, for a
/// finding's message.
struct CoveringDrop<'s> {
    cause: DropCause<'s>,
    index_names: Vec<String>,
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

    fn lines_and_messages_for_change(history: &str, change: &str) -> Vec<(usize, String)> {
        findings_for_change(history, change)
            .into_iter()
            .map(|finding| (finding.start_line, finding.message))
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

    const ORDERS_WITH_FK_COVERED_BY_A_UNIQUE_CONSTRAINT: &str = "
        CREATE TABLE customers (id int PRIMARY KEY);
        CREATE TABLE orders (
            id int PRIMARY KEY,
            customer_id int,
            CONSTRAINT uq_orders_customer UNIQUE (customer_id),
            CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES customers (id)
        );";

    const ORDERS_WITH_FK_COVERED_BY_THE_PRIMARY_KEY: &str = "
        CREATE TABLE customers (id int PRIMARY KEY);
        CREATE TABLE orders (
            id int,
            customer_id int,
            CONSTRAINT orders_pkey PRIMARY KEY (customer_id, id),
            CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES customers (id)
        );";

    const ORDERS_WITH_FK_COVERED_BY_A_TWO_COLUMN_INDEX: &str = "
        CREATE TABLE customers (id int PRIMARY KEY);
        CREATE TABLE orders (
            id int PRIMARY KEY,
            customer_id int,
            legacy_code text,
            CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES customers (id)
        );
        CREATE INDEX idx_orders_customer_legacy ON orders (customer_id, legacy_code);";

    #[rstest]
    #[case::name_another_table_has(
        "customers_pkey",
        &["Foreign key on 'orders(customer_id)' has no covering index."]
    )]
    #[case::free_name("orders_customer_id_idx", &[])]
    fn foreign_key_counts_an_index_created_after_it_unless_postgres_rejects_it(
        #[case] index: &str,
        #[case] expected_without_scan_cost: &[&str],
    ) {
        let messages = messages_for_change(
            "CREATE TABLE customers (id int PRIMARY KEY);
             CREATE TABLE orders (id int PRIMARY KEY, customer_id int);",
            &format!(
                "ALTER TABLE orders ADD CONSTRAINT fk_orders_customer
                     FOREIGN KEY (customer_id) REFERENCES customers (id);
                 CREATE INDEX {index} ON orders (customer_id);"
            ),
        );
        let expected: Vec<String> = expected_without_scan_cost
            .iter()
            .map(|message| format!("{message} {SCAN_COST}"))
            .collect();
        assert_eq!(messages, expected);
    }

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
    #[case::unique(ORDERS_WITH_FK_COVERED_BY_A_UNIQUE_CONSTRAINT, "uq_orders_customer")]
    #[case::primary_key(ORDERS_WITH_FK_COVERED_BY_THE_PRIMARY_KEY, "orders_pkey")]
    fn dropping_a_constraint_whose_index_covered_the_fk_fires(
        #[case] history: &str,
        #[case] constraint_name: &str,
    ) {
        let messages = messages_for_change(
            history,
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
        let messages = messages_for_change(
            ORDERS_WITH_FK_COVERED_BY_A_TWO_COLUMN_INDEX,
            "ALTER TABLE orders DROP COLUMN legacy_code;",
        );
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
        let history = format!(
            "{ORDERS_WITH_FK_COVERED_BY_A_TWO_COLUMN_INDEX}
             CREATE INDEX idx_orders_legacy_customer ON orders (legacy_code, customer_id);"
        );

        let messages = messages_for_change(&history, "ALTER TABLE orders DROP COLUMN legacy_code;");
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
    #[case::index_containing_the_fk_column_anywhere(
        "CREATE INDEX idx_orders_id_customer ON orders (id, customer_id);"
    )]
    #[case::hash_index("CREATE INDEX idx_orders_customer_hash ON orders USING hash (customer_id);")]
    #[case::parenthesized_column(
        "CREATE INDEX idx_orders_customer_paren ON orders ((customer_id));"
    )]
    fn another_index_still_covering_the_fk_is_silent(#[case] second_index: &str) {
        let history = format!("{ORDERS_WITH_COVERED_FK}\n{second_index}");
        let messages = messages_for_change(&history, "DROP INDEX idx_orders_customer_id;");
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[rstest]
    #[case::partial_index(
        "CREATE INDEX idx_orders_customer_open ON orders (customer_id) WHERE id > 0;"
    )]
    #[case::gin_index("CREATE INDEX idx_orders_customer_gin ON orders USING gin (customer_id);")]
    #[case::parenthesized_expression(
        "CREATE INDEX idx_orders_customer_plus ON orders ((customer_id + 0));"
    )]
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

        let reported = lines_and_messages_for_change(
            &history,
            "DROP INDEX idx_orders_customer_id;\nDROP INDEX idx_orders_customer_id_id;",
        );
        assert_eq!(
            reported,
            [(
                2,
                format!(
                    "Dropping index 'idx_orders_customer_id_id' leaves foreign key \
                     'fk_orders_customer' on 'orders(customer_id)' without a covering index. \
                     {SCAN_COST}"
                )
            )]
        );
    }

    #[test]
    fn drop_index_of_a_constraints_index_is_silent_as_postgres_rejects_it() {
        let messages = messages_for_change(
            ORDERS_WITH_FK_COVERED_BY_A_UNIQUE_CONSTRAINT,
            "DROP INDEX uq_orders_customer;",
        );
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[rstest]
    #[case::index_on_another_column("CREATE INDEX idx_orders_id ON orders (id);", "idx_orders_id")]
    #[case::fk_never_covered(
        "CREATE INDEX idx_customers_id ON customers (id);",
        "idx_customers_id"
    )]
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

    #[test]
    fn index_renamed_then_dropped_fires_on_the_drop_under_its_new_name() {
        let reported = lines_and_messages_for_change(
            ORDERS_WITH_COVERED_FK,
            "ALTER INDEX idx_orders_customer_id RENAME TO idx_orders_customer_id_old;
             DROP INDEX idx_orders_customer_id_old;",
        );
        assert_eq!(
            reported,
            [(
                2,
                format!(
                    "Dropping index 'idx_orders_customer_id_old' leaves foreign key \
                     'fk_orders_customer' on 'orders(customer_id)' without a covering index. \
                     {SCAN_COST}"
                )
            )]
        );
    }

    #[rstest]
    #[case::unique_renamed_by_rename_constraint(
        ORDERS_WITH_FK_COVERED_BY_A_UNIQUE_CONSTRAINT,
        "ALTER TABLE orders RENAME CONSTRAINT uq_orders_customer TO uq_orders_customer_old;",
        "uq_orders_customer_old"
    )]
    #[case::unique_renamed_by_alter_index(
        ORDERS_WITH_FK_COVERED_BY_A_UNIQUE_CONSTRAINT,
        "ALTER INDEX uq_orders_customer RENAME TO uq_orders_customer_old;",
        "uq_orders_customer_old"
    )]
    #[case::primary_key_renamed_by_rename_constraint(
        ORDERS_WITH_FK_COVERED_BY_THE_PRIMARY_KEY,
        "ALTER TABLE orders RENAME CONSTRAINT orders_pkey TO orders_pkey_old;",
        "orders_pkey_old"
    )]
    #[case::primary_key_renamed_by_alter_index(
        ORDERS_WITH_FK_COVERED_BY_THE_PRIMARY_KEY,
        "ALTER INDEX orders_pkey RENAME TO orders_pkey_old;",
        "orders_pkey_old"
    )]
    fn index_backed_constraint_renamed_then_dropped_fires_on_the_drop_under_its_new_name(
        #[case] history: &str,
        #[case] rename: &str,
        #[case] new_name: &str,
    ) {
        let reported = lines_and_messages_for_change(
            history,
            &format!(
                "{rename}
                 ALTER TABLE orders DROP CONSTRAINT {new_name};"
            ),
        );
        assert_eq!(
            reported,
            [(
                2,
                format!(
                    "Dropping constraint '{new_name}' drops its index, which leaves foreign key \
                     'fk_orders_customer' on 'orders(customer_id)' without a covering index. \
                     {SCAN_COST}"
                )
            )]
        );
    }

    #[test]
    fn index_created_renamed_and_dropped_in_the_unit_takes_the_finding_when_it_covered_last() {
        let reported = lines_and_messages_for_change(
            ORDERS_WITH_COVERED_FK,
            "CREATE INDEX idx_orders_customer_tmp ON orders (customer_id);
             DROP INDEX idx_orders_customer_id;
             ALTER INDEX idx_orders_customer_tmp RENAME TO idx_orders_customer_v2;
             DROP INDEX idx_orders_customer_v2;",
        );
        assert_eq!(
            reported,
            [(
                4,
                format!(
                    "Dropping index 'idx_orders_customer_v2' leaves foreign key \
                     'fk_orders_customer' on 'orders(customer_id)' without a covering index. \
                     {SCAN_COST}"
                )
            )]
        );
    }

    #[test]
    fn index_dropped_recreated_and_dropped_again_reports_the_second_drop() {
        let reported = lines_and_messages_for_change(
            ORDERS_WITH_COVERED_FK,
            "DROP INDEX idx_orders_customer_id;
             CREATE INDEX idx_orders_customer_id ON orders (customer_id);
             DROP INDEX idx_orders_customer_id;",
        );
        assert_eq!(
            reported,
            [(
                3,
                format!(
                    "Dropping index 'idx_orders_customer_id' leaves foreign key \
                     'fk_orders_customer' on 'orders(customer_id)' without a covering index. \
                     {SCAN_COST}"
                )
            )]
        );
    }

    #[rstest]
    #[case::rename_then_drop_with_another_index_covering(
        "CREATE INDEX idx_orders_customer_created ON orders (customer_id, id);",
        "ALTER INDEX idx_orders_customer_id RENAME TO idx_orders_customer_id_old;
         DROP INDEX idx_orders_customer_id_old;"
    )]
    #[case::rename_drop_then_create_a_replacement(
        "",
        "ALTER INDEX idx_orders_customer_id RENAME TO idx_orders_customer_id_old;
         DROP INDEX idx_orders_customer_id_old;
         CREATE INDEX idx_orders_customer_id ON orders (customer_id);"
    )]
    #[case::rename_create_a_replacement_then_drop(
        "",
        "ALTER INDEX idx_orders_customer_id RENAME TO idx_orders_customer_id_old;
         CREATE INDEX idx_orders_customer_id ON orders (customer_id);
         DROP INDEX idx_orders_customer_id_old;"
    )]
    #[case::drop_then_recreate(
        "",
        "DROP INDEX idx_orders_customer_id;
         CREATE INDEX idx_orders_customer_id ON orders (customer_id);"
    )]
    #[case::rename_of_the_covering_index_alone(
        "",
        "ALTER INDEX idx_orders_customer_id RENAME TO idx_orders_customer;"
    )]
    #[case::unrelated_index_renamed_then_dropped(
        "CREATE INDEX idx_orders_id ON orders (id);",
        "ALTER INDEX idx_orders_id RENAME TO idx_orders_id_old;
         DROP INDEX idx_orders_id_old;"
    )]
    fn fk_still_covered_at_the_end_of_the_unit_is_silent(
        #[case] more_history: &str,
        #[case] change: &str,
    ) {
        let history = format!("{ORDERS_WITH_COVERED_FK}\n{more_history}");
        let messages = messages_for_change(&history, change);
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[test]
    fn index_created_and_dropped_in_the_unit_is_silent_when_the_fk_had_no_coverage_before() {
        let history = "
            CREATE TABLE customers (id int PRIMARY KEY);
            CREATE TABLE orders (
                id int PRIMARY KEY,
                customer_id int,
                CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES customers (id)
            );";

        let messages = messages_for_change(
            history,
            "CREATE INDEX idx_orders_customer_backfill ON orders (customer_id);
             UPDATE orders SET customer_id = customer_id WHERE customer_id IS NULL;
             DROP INDEX idx_orders_customer_backfill;",
        );
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[test]
    fn partitioned_parents_index_renamed_then_dropped_fires_for_a_child_fk_it_covered() {
        let history = "
            CREATE TABLE refs (id int PRIMARY KEY);
            CREATE TABLE orders (id int, ref_id int) PARTITION BY RANGE (id);
            CREATE INDEX idx_orders_ref_id ON orders (ref_id);
            CREATE TABLE orders_2024 PARTITION OF orders FOR VALUES FROM (0) TO (100);
            ALTER TABLE orders_2024 ADD CONSTRAINT fk_ref FOREIGN KEY (ref_id) REFERENCES refs (id);";

        let messages = messages_for_change(
            history,
            "ALTER INDEX idx_orders_ref_id RENAME TO idx_orders_ref_id_old;
             DROP INDEX idx_orders_ref_id_old;",
        );
        assert_eq!(
            messages,
            [format!(
                "Dropping index 'idx_orders_ref_id_old' leaves foreign key 'fk_ref' on \
                 'orders_2024(ref_id)' without a covering index. {SCAN_COST}"
            )]
        );
    }

    #[test]
    fn drop_that_leaves_the_parents_coverage_is_silent_though_a_later_detach_ends_it() {
        let history = "
            CREATE TABLE refs (id int PRIMARY KEY);
            CREATE TABLE orders (id int, ref_id int) PARTITION BY RANGE (id);
            CREATE INDEX idx_orders_ref_id ON orders (ref_id);
            CREATE TABLE orders_2024 PARTITION OF orders FOR VALUES FROM (0) TO (100);
            CREATE INDEX idx_orders_2024_ref_id ON orders_2024 (ref_id);
            ALTER TABLE orders_2024 ADD CONSTRAINT fk_ref FOREIGN KEY (ref_id) REFERENCES refs (id);";

        let messages = messages_for_change(
            history,
            "DROP INDEX idx_orders_2024_ref_id;
             ALTER TABLE orders DETACH PARTITION orders_2024;",
        );
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[test]
    fn index_renamed_then_dropped_with_its_column_is_named_by_its_new_name() {
        let reported = lines_and_messages_for_change(
            ORDERS_WITH_FK_COVERED_BY_A_TWO_COLUMN_INDEX,
            "ALTER INDEX idx_orders_customer_legacy RENAME TO idx_orders_customer_legacy_old;
             ALTER TABLE orders DROP COLUMN legacy_code;",
        );
        assert_eq!(
            reported,
            [(
                2,
                format!(
                    "Dropping column 'legacy_code' drops index 'idx_orders_customer_legacy_old', \
                     which leaves foreign key 'fk_orders_customer' on 'orders(customer_id)' \
                     without a covering index. {SCAN_COST}"
                )
            )]
        );
    }

    #[test]
    fn constraint_added_using_the_covering_index_then_dropped_fires_on_the_drop() {
        let history = "
            CREATE TABLE customers (id int PRIMARY KEY);
            CREATE TABLE orders (
                id int PRIMARY KEY,
                customer_id int,
                CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES customers (id)
            );
            CREATE UNIQUE INDEX idx_orders_customer_id ON orders (customer_id);";

        let reported = lines_and_messages_for_change(
            history,
            "ALTER TABLE orders ADD CONSTRAINT uq_orders_customer
                 UNIQUE USING INDEX idx_orders_customer_id;
             ALTER TABLE orders DROP CONSTRAINT uq_orders_customer;",
        );
        assert_eq!(
            reported,
            [(
                3,
                format!(
                    "Dropping constraint 'uq_orders_customer' drops its index, which leaves \
                     foreign key 'fk_orders_customer' on 'orders(customer_id)' without a \
                     covering index. {SCAN_COST}"
                )
            )]
        );
    }

    #[test]
    fn rejected_statement_ending_coverage_reports_nothing_though_an_earlier_drop_took_an_index() {
        let history = "
            CREATE TABLE customers (id int PRIMARY KEY);
            CREATE TABLE orders (
                id int PRIMARY KEY,
                customer_id int,
                code text CONSTRAINT uq_orders_code UNIQUE,
                CONSTRAINT fk_orders_customer FOREIGN KEY (customer_id) REFERENCES customers (id)
            );
            CREATE INDEX idx_orders_customer_id ON orders (customer_id);
            CREATE INDEX idx_orders_customer_id_id ON orders (customer_id, id);";
        let earlier_drop = "DROP INDEX idx_orders_customer_id;";

        let rejected = lines_and_messages_for_change(
            history,
            &format!("{earlier_drop}\nDROP INDEX idx_orders_customer_id_id, uq_orders_code;"),
        );
        assert!(rejected.is_empty(), "got {rejected:?}");

        let accepted = lines_and_messages_for_change(
            history,
            &format!("{earlier_drop}\nDROP INDEX idx_orders_customer_id_id;"),
        );
        assert_eq!(
            accepted,
            [(
                2,
                format!(
                    "Dropping index 'idx_orders_customer_id_id' leaves foreign key \
                     'fk_orders_customer' on 'orders(customer_id)' without a covering index. \
                     {SCAN_COST}"
                )
            )]
        );
    }
}
