//! Statements PostgreSQL rejects against the schema the catalog holds just
//! before them. PGM027 reports them, and the rules that would report what
//! such a statement does leave it to PGM027 ([`RejectedStatements`]), as long
//! as PGM027 judges the unit ([`unit_is_judged`]).
//!
//! Each statement is judged against `catalog_before` with the unit's earlier
//! statements replayed on top ([`map_with_state_before_if_needed`]), so a
//! name an earlier statement in the same unit frees or takes counts. Only
//! positive evidence rejects a statement: an object the catalog does not
//! have may exist outside the tracked migrations.

use std::cell::OnceCell;

use crate::catalog::Catalog;
use crate::catalog::replay::{map_with_state_before_if_needed, relation_holding_index_name};
use crate::catalog::types::{NamedRelation, RenameClash};
use crate::names::{RelationName, ResolvedName};
use crate::parser::ir::{AlterTable, CreateIndex, IrNode, Located, SourceSpan};
use crate::rules::LintContext;
use crate::rules::using_index::{UsingIndex, has_using_index};

/// True when the unit of `ctx` is judged for statements PostgreSQL rejects:
/// PGM027 reports them, and the rules that leave them to PGM027 skip them.
/// When it is not, PGM027 reports nothing and no rule skips a statement
/// because PGM027 would report it: each reports the statement as it would a
/// statement PostgreSQL accepts, which may be nothing. No unit is judged
/// while PGM027 is disabled
/// (`ctx.pgm027_enabled`). Nor is a down migration. A go-migrate down file
/// sorts before its up file, so replay applies it first and judges it
/// against a schema it was never meant to run on: names its up file drops
/// still look taken, and names it creates do not exist yet.
pub(super) fn unit_is_judged(ctx: &LintContext<'_>) -> bool {
    ctx.pgm027_enabled && !ctx.is_down
}

/// The statements of a unit PostgreSQL rejects, as [`rejections`] judges
/// them, for a rule that leaves those statements to PGM027. The unit is
/// judged on the first [`RejectedStatements::contains`], so a rule that asks
/// only before reporting replays nothing when it has nothing to report.
pub(super) struct RejectedStatements<'a> {
    catalog_before: &'a Catalog,
    statements: &'a [Located<IrNode>],
    unit_is_judged: bool,
    rejected_spans: OnceCell<Vec<&'a SourceSpan>>,
}

impl<'a> RejectedStatements<'a> {
    /// The rejected statements among `statements`, the unit of `ctx`, judged
    /// against `ctx.catalog_before` once a rule asks. Empty when the unit is
    /// not judged ([`unit_is_judged`]).
    pub(super) fn new(ctx: &LintContext<'a>, statements: &'a [Located<IrNode>]) -> Self {
        Self {
            catalog_before: ctx.catalog_before,
            statements,
            unit_is_judged: unit_is_judged(ctx),
            rejected_spans: OnceCell::new(),
        }
    }

    /// True when PostgreSQL rejects the statement at `span`. `DROP INDEX a, b`
    /// is one statement split into one node per index, all with the
    /// statement's span, and PostgreSQL rejects all of it when it rejects one
    /// index (`verify/tests/other/pgm027_drop_constraint_index.sql`).
    pub(super) fn contains(&self, span: &SourceSpan) -> bool {
        if !self.unit_is_judged {
            return false;
        }
        self.rejected_spans
            .get_or_init(|| {
                self.statements
                    .iter()
                    .zip(rejections(self.catalog_before, self.statements))
                    .filter(|(_, rejection)| rejection.is_some())
                    .map(|(stmt, _)| &stmt.span)
                    .collect()
            })
            .contains(&span)
    }
}

/// Why PostgreSQL rejects a statement, with the names a message needs. Table
/// names are display names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Rejection {
    /// `ADD PRIMARY KEY` or `ADD UNIQUE ... USING INDEX index` on `table`,
    /// where `index` is in `table`'s schema but belongs to `owner`
    /// (`verify/tests/other/pgm027_using_index_of_another_table.sql`).
    UsingIndexOfAnotherTable {
        constraint_kind: &'static str,
        index: String,
        table: String,
        owner: String,
    },
    /// A rename onto a name that is taken where the renamed object needs it
    /// free (`verify/tests/naming/rename_taken.sql`).
    RenameOntoTakenName {
        renamed: Renamed,
        new_name: String,
        taken_by: TakenBy,
    },
    /// `DROP INDEX index` where `index` backs `constraint` on `table`
    /// (`verify/tests/other/pgm027_drop_constraint_index.sql`).
    DropIndexOfConstraint {
        index: String,
        constraint: String,
        table: String,
    },
    /// `CREATE INDEX index ON table` without `IF NOT EXISTS`, where a
    /// relation in `table`'s schema already has the name `index`
    /// (`verify/tests/other/pgm027_create_index_onto_taken_name.sql`).
    CreateIndexOntoTakenName {
        index: String,
        table: String,
        taken_by: TakenBy,
    },
}

/// The object a rejected rename renames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Renamed {
    /// `ALTER TABLE table RENAME TO`.
    Table { table: String },
    /// `ALTER INDEX index RENAME TO`.
    Index { index: String },
    /// `ALTER TABLE table RENAME CONSTRAINT constraint TO`.
    Constraint { constraint: String, table: String },
}

/// What already has the name a rejected rename or `CREATE INDEX` wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TakenBy {
    /// The table `table`.
    Table { table: String },
    /// An index of that name on `table`.
    Index { table: String },
    /// A constraint of that name on `table`, the renamed object's table.
    Constraint { table: String },
}

impl TakenBy {
    /// The relation `holder`, which has the name.
    fn relation(holder: NamedRelation<'_>) -> Self {
        match holder {
            NamedRelation::Table(table) => Self::Table {
                table: table.display_name.clone(),
            },
            NamedRelation::Index { table } => Self::Index {
                table: table.display_name.clone(),
            },
        }
    }
}

/// For each of `statements`, why PostgreSQL rejects it, judged against
/// `catalog_before` with the earlier statements of `statements` replayed on
/// top. `None` when the catalog gives no reason to.
pub(super) fn rejections(
    catalog_before: &Catalog,
    statements: &[Located<IrNode>],
) -> Vec<Option<Rejection>> {
    map_with_state_before_if_needed(
        catalog_before,
        statements,
        can_be_rejected,
        |state, stmt| rejection(state, &stmt.node),
    )
}

/// True for the statements [`rejection`] can reject, so a unit without any
/// needs no replay: [`rejection`] returns `None` for every other statement,
/// whatever the catalog holds.
fn can_be_rejected(node: &IrNode) -> bool {
    match node {
        IrNode::RenameTable { .. }
        | IrNode::RenameIndex { .. }
        | IrNode::RenameConstraint { .. }
        | IrNode::DropIndex(_) => true,
        IrNode::CreateIndex(ci) => !ci.if_not_exists && ci.index_name.is_some(),
        _ => has_using_index(node),
    }
}

/// Why PostgreSQL rejects `node` on top of `state`, if the catalog shows it.
fn rejection(state: &Catalog, node: &IrNode) -> Option<Rejection> {
    match node {
        IrNode::AlterTable(at) => using_index_of_another_table(state, at),
        IrNode::RenameTable { name, new_name } => {
            let clash = state.table_rename_clash(name.relation(), new_name)?;
            rename_onto_taken_name(
                state,
                Renamed::Table {
                    table: name.display_name(),
                },
                name.relation(),
                new_name,
                clash,
            )
        }
        IrNode::RenameIndex { index, new_name } => {
            let clash = state.index_rename_clash(index.relation(), new_name)?;
            let table = state.table_for_index(index.relation())?;
            rename_onto_taken_name(
                state,
                Renamed::Index {
                    index: index.display_name(),
                },
                table,
                new_name,
                clash,
            )
        }
        IrNode::RenameConstraint {
            table,
            old_name,
            new_name,
        } => {
            let clash = state.constraint_rename_clash(table.relation(), old_name, new_name)?;
            rename_onto_taken_name(
                state,
                Renamed::Constraint {
                    constraint: old_name.clone(),
                    table: table.display_name(),
                },
                table.relation(),
                new_name,
                clash,
            )
        }
        IrNode::DropIndex(di) => drop_index_of_constraint(state, &di.index_name),
        IrNode::CreateIndex(ci) if !ci.if_not_exists => create_index_onto_taken_name(state, ci),
        _ => None,
    }
}

/// The first `USING INDEX` in `at` that names an index of another table, as
/// [`UsingIndex::in_catalog`] tells it for PGM016, PGM017 and PGM107 too. It
/// asks [`Catalog::other_table_owning_index`], as replay does to leave the
/// statement out.
fn using_index_of_another_table(state: &Catalog, at: &AlterTable) -> Option<Rejection> {
    at.using_index_constraints()
        .find_map(|(constraint_kind, index)| {
            let UsingIndex::OfAnotherTable { owner } =
                UsingIndex::in_catalog(state, at.name.relation(), index)
            else {
                return None;
            };
            Some(Rejection::UsingIndexOfAnotherTable {
                constraint_kind,
                index: index.to_string(),
                table: at.name.display_name(),
                owner: owner.to_string(),
            })
        })
}

/// The rejection of a rename of `renamed` to `new_name`, which `clash`
/// says is taken. `table` is the table the renamed object is or belongs to:
/// a relation name is looked up in its schema, and a constraint name on it.
fn rename_onto_taken_name(
    state: &Catalog,
    renamed: Renamed,
    table: &RelationName,
    new_name: &str,
    clash: RenameClash,
) -> Option<Rejection> {
    let taken_by = match clash {
        RenameClash::Relation => {
            TakenBy::relation(state.relation_named(&table.in_same_schema(new_name))?)
        }
        RenameClash::Constraint => TakenBy::Constraint {
            table: state.get_table(table)?.display_name.clone(),
        },
    };
    Some(Rejection::RenameOntoTakenName {
        renamed,
        new_name: new_name.to_string(),
        taken_by,
    })
}

/// The rejection of `DROP INDEX index` when `index` backs a PRIMARY KEY,
/// UNIQUE or EXCLUDE constraint of its table. Only a registered index
/// counts, so an EXCLUDE constraint's, which the catalog does not register,
/// does not yet.
fn drop_index_of_constraint(state: &Catalog, index: &ResolvedName) -> Option<Rejection> {
    let table = state.get_table(state.table_for_index(index.relation())?)?;
    let constraint = table.constraint_backed_by(index.name())?;
    Some(Rejection::DropIndexOfConstraint {
        index: index.display_name(),
        constraint: constraint.name().to_string(),
        table: table.display_name.clone(),
    })
}

/// The rejection of a `CREATE INDEX` without `IF NOT EXISTS` whose name a
/// relation in its table's schema already has, as
/// [`relation_holding_index_name`] tells it for replay too, which leaves the
/// statement out. With `IF NOT EXISTS` PostgreSQL skips the statement
/// instead.
fn create_index_onto_taken_name(state: &Catalog, ci: &CreateIndex) -> Option<Rejection> {
    let holder = relation_holding_index_name(state, ci)?;
    Some(Rejection::CreateIndexOntoTakenName {
        index: ci.index_name.clone()?,
        table: ci.table_name.display_name(),
        taken_by: TakenBy::relation(holder),
    })
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use crate::parser::pg_query::parse_sql;
    use crate::rules::test_helpers::{catalog_from_sql, lint_ctx, make_ctx, normalized};
    use crate::rules::{LintContext, Rule, RuleId};

    const ORDERS_WITH_CONSTRAINTS_AND_A_COVERED_FK: &str = "
        CREATE TABLE customers (id bigint PRIMARY KEY);
        CREATE TABLE orders (
            id bigint PRIMARY KEY,
            code text CONSTRAINT orders_code_key UNIQUE,
            customer_id bigint CONSTRAINT orders_customer_fk REFERENCES customers (id));
        CREATE INDEX orders_customer_id_idx ON orders (customer_id);
        CREATE TABLE archive (id bigint PRIMARY KEY);
        CREATE TABLE ledger (id int, code text);
        CREATE UNIQUE INDEX ledger_id_key ON ledger (id);";

    fn messages_for_change(rule: RuleId, change: &str) -> Vec<String> {
        let history = ORDERS_WITH_CONSTRAINTS_AND_A_COVERED_FK;
        let before = catalog_from_sql(history);
        let after = catalog_from_sql(&format!("{history}\n{change}"));
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");
        rule.check(&normalized(parse_sql(change)), &ctx)
            .into_iter()
            .map(|finding| finding.message)
            .collect()
    }

    fn messages_for_down_change(rule: RuleId, change: &str) -> Vec<String> {
        let history = ORDERS_WITH_CONSTRAINTS_AND_A_COVERED_FK;
        let before = catalog_from_sql(history);
        let after = catalog_from_sql(&format!("{history}\n{change}"));
        let file = std::path::PathBuf::from("migrations/000002_orders.down.sql");
        let created = std::collections::HashSet::new();
        let ctx = LintContext {
            is_down: true,
            ..make_ctx(&before, &after, &file, &created)
        };
        rule.check(&normalized(parse_sql(change)), &ctx)
            .into_iter()
            .map(|finding| finding.message)
            .collect()
    }

    fn messages_for_change_with_pgm027_disabled(rule: RuleId, change: &str) -> Vec<String> {
        let history = ORDERS_WITH_CONSTRAINTS_AND_A_COVERED_FK;
        let before = catalog_from_sql(history);
        let after = catalog_from_sql(&format!("{history}\n{change}"));
        lint_ctx!(ctx, &before, &after, "migrations/002.sql", pgm027_enabled: false);
        rule.check(&normalized(parse_sql(change)), &ctx)
            .into_iter()
            .map(|finding| finding.message)
            .collect()
    }

    #[rstest]
    #[case::pgm002(
        RuleId::Pgm002,
        "DROP INDEX orders_pkey;",
        "DROP INDEX orders_customer_id_idx;"
    )]
    #[case::pgm002_constraint_index_named_with_a_plain_one(
        RuleId::Pgm002,
        "DROP INDEX orders_customer_id_idx, orders_code_key;",
        "DROP INDEX orders_customer_id_idx;"
    )]
    #[case::pgm003(
        RuleId::Pgm003,
        "DROP INDEX CONCURRENTLY orders_code_key;",
        "DROP INDEX CONCURRENTLY orders_customer_id_idx;"
    )]
    #[case::pgm401(
        RuleId::Pgm401,
        "DROP INDEX orders_pkey;",
        "DROP INDEX orders_customer_id_idx;"
    )]
    #[case::pgm501_constraint_index_named_with_the_covering_one(
        RuleId::Pgm501,
        "DROP INDEX orders_customer_id_idx, orders_code_key;",
        "DROP INDEX orders_customer_id_idx;"
    )]
    #[case::pgm504(
        RuleId::Pgm504,
        "ALTER TABLE archive RENAME TO customers;",
        "ALTER TABLE archive RENAME TO archive_old;"
    )]
    #[case::pgm402(
        RuleId::Pgm402,
        "CREATE INDEX archive ON orders (code);",
        "CREATE INDEX orders_code_idx ON orders (code);"
    )]
    #[case::pgm508(
        RuleId::Pgm508,
        "CREATE INDEX archive_pkey ON orders (customer_id);",
        "CREATE INDEX orders_customer_idx ON orders (customer_id);"
    )]
    #[case::pgm016(
        RuleId::Pgm016,
        "ALTER TABLE ledger ADD PRIMARY KEY USING INDEX customers_pkey;",
        "ALTER TABLE ledger ADD PRIMARY KEY USING INDEX ledger_id_key;"
    )]
    #[case::pgm017(
        RuleId::Pgm017,
        "ALTER TABLE ledger ADD UNIQUE USING INDEX orders_customer_id_idx;",
        "ALTER TABLE orders ADD UNIQUE USING INDEX orders_customer_id_idx;"
    )]
    #[case::pgm107(
        RuleId::Pgm107,
        "ALTER TABLE ledger ADD PRIMARY KEY USING INDEX customers_pkey;",
        "ALTER TABLE ledger ADD PRIMARY KEY USING INDEX ledger_id_key;"
    )]
    fn rule_leaves_a_statement_postgres_rejects_to_pgm027(
        #[case] rule: RuleId,
        #[case] rejected: &str,
        #[case] accepted_of_the_same_shape: &str,
    ) {
        let rule_on_rejected = messages_for_change(rule, rejected);
        assert!(rule_on_rejected.is_empty(), "got {rule_on_rejected:?}");
        assert_eq!(messages_for_change(RuleId::Pgm027, rejected).len(), 1);

        let rule_on_accepted = messages_for_change(rule, accepted_of_the_same_shape);
        assert_eq!(rule_on_accepted.len(), 1, "got {rule_on_accepted:?}");
        let pgm027_on_accepted = messages_for_change(RuleId::Pgm027, accepted_of_the_same_shape);
        assert!(pgm027_on_accepted.is_empty(), "got {pgm027_on_accepted:?}");
    }

    #[rstest]
    #[case::pgm002(RuleId::Pgm002, "DROP INDEX orders_pkey;")]
    #[case::pgm003(RuleId::Pgm003, "DROP INDEX CONCURRENTLY orders_pkey;")]
    #[case::pgm401(RuleId::Pgm401, "DROP INDEX orders_pkey;")]
    #[case::pgm402(RuleId::Pgm402, "CREATE INDEX archive ON orders (code);")]
    #[case::pgm501(RuleId::Pgm501, "DROP INDEX orders_customer_id_idx, orders_code_key;")]
    #[case::pgm504(RuleId::Pgm504, "ALTER TABLE archive RENAME TO customers;")]
    #[case::pgm016(
        RuleId::Pgm016,
        "ALTER TABLE ledger ADD PRIMARY KEY USING INDEX customers_pkey;"
    )]
    #[case::pgm017(
        RuleId::Pgm017,
        "ALTER TABLE ledger ADD UNIQUE USING INDEX orders_customer_id_idx;"
    )]
    #[case::pgm107(
        RuleId::Pgm107,
        "ALTER TABLE ledger ADD PRIMARY KEY USING INDEX customers_pkey;"
    )]
    fn rule_reports_a_statement_of_a_down_migration_that_pgm027_does_not_judge(
        #[case] rule: RuleId,
        #[case] rejected_outside_a_down_migration: &str,
    ) {
        let rule_on_down = messages_for_down_change(rule, rejected_outside_a_down_migration);
        assert_eq!(rule_on_down.len(), 1, "got {rule_on_down:?}");
        let pgm027_on_down =
            messages_for_down_change(RuleId::Pgm027, rejected_outside_a_down_migration);
        assert!(pgm027_on_down.is_empty(), "got {pgm027_on_down:?}");
    }

    #[rstest]
    #[case::pgm002(RuleId::Pgm002, "DROP INDEX orders_pkey;", 1)]
    #[case::pgm002_one_finding_per_index_named(
        RuleId::Pgm002,
        "DROP INDEX orders_customer_id_idx, orders_code_key;",
        2
    )]
    #[case::pgm003(RuleId::Pgm003, "DROP INDEX CONCURRENTLY orders_code_key;", 1)]
    #[case::pgm401(RuleId::Pgm401, "DROP INDEX orders_pkey;", 1)]
    #[case::pgm501_constraint_index_named_with_the_covering_one(
        RuleId::Pgm501,
        "DROP INDEX orders_customer_id_idx, orders_code_key;",
        1
    )]
    #[case::pgm504(RuleId::Pgm504, "ALTER TABLE archive RENAME TO customers;", 1)]
    #[case::pgm402(RuleId::Pgm402, "CREATE INDEX archive ON orders (code);", 1)]
    #[case::pgm016(
        RuleId::Pgm016,
        "ALTER TABLE ledger ADD PRIMARY KEY USING INDEX customers_pkey;",
        1
    )]
    #[case::pgm017(
        RuleId::Pgm017,
        "ALTER TABLE ledger ADD UNIQUE USING INDEX orders_customer_id_idx;",
        1
    )]
    #[case::pgm107(
        RuleId::Pgm107,
        "ALTER TABLE ledger ADD PRIMARY KEY USING INDEX customers_pkey;",
        1
    )]
    fn rule_reports_a_statement_postgres_rejects_while_pgm027_is_disabled(
        #[case] rule: RuleId,
        #[case] rejected: &str,
        #[case] expected_findings: usize,
    ) {
        let rule_on_rejected = messages_for_change_with_pgm027_disabled(rule, rejected);
        assert_eq!(
            rule_on_rejected.len(),
            expected_findings,
            "got {rule_on_rejected:?}"
        );
    }

    #[test]
    fn pgm001_reports_create_index_onto_a_taken_name_alongside_pgm027() {
        let change = "CREATE INDEX archive_pkey ON orders (code);";

        assert_eq!(messages_for_change(RuleId::Pgm001, change).len(), 1);
        assert_eq!(messages_for_change(RuleId::Pgm027, change).len(), 1);
    }

    #[test]
    fn pgm003_reports_create_index_concurrently_onto_a_taken_name_alongside_pgm027() {
        let change = "CREATE INDEX CONCURRENTLY archive_pkey ON orders (code);";

        assert_eq!(messages_for_change(RuleId::Pgm003, change).len(), 1);
        assert_eq!(messages_for_change(RuleId::Pgm027, change).len(), 1);
    }
}
