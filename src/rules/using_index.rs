//! What the index named in `ALTER TABLE table ADD PRIMARY KEY USING INDEX
//! index` or `ADD UNIQUE USING INDEX index` refers to.
//!
//! PostgreSQL looks the name up in the altered table's schema when the
//! statement runs, so an index created, dropped or renamed earlier in the
//! same migration unit counts. Attaching the index renames it to the
//! constraint's name, so `catalog_after` may no longer have it under the name
//! the statement uses. Each statement is therefore judged against
//! `catalog_before` with the unit's earlier statements replayed on top
//! ([`map_with_state_before`]). PGM027 judges a `USING INDEX` the same way
//! with the same [`UsingIndex::in_catalog`], so PGM016, PGM017 and PGM107
//! never see an index of the altered table where PGM027 reports another
//! table's.

use crate::catalog::Catalog;
use crate::catalog::replay::map_with_state_before;
use crate::catalog::types::{IndexState, TableState};
use crate::names::RelationName;
use crate::parser::ir::{AlterTable, AlterTableAction, IrNode, Located, TableConstraint};

/// What the index name in `ALTER TABLE ... ADD ... USING INDEX` refers to in
/// a catalog.
#[derive(Debug, Clone)]
pub(super) enum UsingIndex {
    /// An index of the altered table, with the table as the catalog holds it.
    OfTable {
        index: IndexState,
        table: TableState,
    },
    /// An index of another table in the altered table's schema. PostgreSQL
    /// rejects the statement, which PGM027 reports. `owner` is that table's
    /// display name.
    OfAnotherTable { owner: String },
    /// No index of that name in the altered table's schema, as far as the
    /// catalog knows.
    Missing,
}

impl UsingIndex {
    /// What `index_name` refers to in `ALTER TABLE table ADD ... USING INDEX
    /// index_name` against `catalog`. PostgreSQL looks the name up in
    /// `table`'s schema. The altered table need not be in the catalog for an
    /// index of another table: whichever table it is, it is not the index's.
    pub(super) fn in_catalog(catalog: &Catalog, table: &RelationName, index_name: &str) -> Self {
        let index_key = table.in_same_schema(index_name);
        let Some(owner_key) = catalog.table_for_index(&index_key) else {
            return Self::Missing;
        };
        let Some(owner) = catalog.get_table(owner_key) else {
            return Self::Missing;
        };
        if owner_key != table {
            return Self::OfAnotherTable {
                owner: owner.display_name.clone(),
            };
        }
        match owner.indexes.iter().find(|idx| idx.name == index_name) {
            Some(index) => Self::OfTable {
                index: index.clone(),
                table: owner.clone(),
            },
            None => Self::Missing,
        }
    }
}

/// What the indexes of one statement's `USING INDEX` constraints refer to,
/// just before the statement.
#[derive(Debug, Default)]
pub(super) struct StatementUsingIndexes<'s> {
    by_index_name: Vec<(&'s str, UsingIndex)>,
}

impl StatementUsingIndexes<'_> {
    /// What `index_name`, named by one of the statement's `USING INDEX`
    /// constraints, refers to. `None` for a name the statement does not use
    /// that way.
    pub(super) fn get(&self, index_name: &str) -> Option<&UsingIndex> {
        self.by_index_name
            .iter()
            .find(|(name, _)| *name == index_name)
            .map(|(_, using_index)| using_index)
    }
}

/// For each of `statements`, what the index of each of its `USING INDEX`
/// constraints refers to, judged against `catalog_before` with the earlier
/// statements of `statements` replayed on top. A unit without a `USING
/// INDEX` needs no replay.
pub(super) fn using_indexes_by_statement<'s>(
    catalog_before: &Catalog,
    statements: &'s [Located<IrNode>],
) -> Vec<StatementUsingIndexes<'s>> {
    if !statements.iter().any(|stmt| has_using_index(&stmt.node)) {
        return statements
            .iter()
            .map(|_| StatementUsingIndexes::default())
            .collect();
    }
    map_with_state_before(catalog_before, statements, |state, stmt| {
        let IrNode::AlterTable(at) = &stmt.node else {
            return StatementUsingIndexes::default();
        };
        let table = at.name.relation();
        StatementUsingIndexes {
            by_index_name: using_index_constraints(at)
                .map(|(_, index_name)| {
                    (index_name, UsingIndex::in_catalog(state, table, index_name))
                })
                .collect(),
        }
    })
}

/// True for an `ALTER TABLE` with a `USING INDEX` constraint.
pub(super) fn has_using_index(node: &IrNode) -> bool {
    matches!(node, IrNode::AlterTable(at) if using_index_constraints(at).next().is_some())
}

/// The `(constraint kind, index name)` of each `ADD PRIMARY KEY` or
/// `ADD UNIQUE ... USING INDEX` in `at`, in statement order.
pub(super) fn using_index_constraints(
    at: &AlterTable,
) -> impl Iterator<Item = (&'static str, &str)> {
    at.actions.iter().filter_map(|action| match action {
        AlterTableAction::AddConstraint {
            constraint:
                TableConstraint::PrimaryKey {
                    using_index: Some(index),
                    ..
                },
            ..
        } => Some(("PRIMARY KEY", index.as_str())),
        AlterTableAction::AddConstraint {
            constraint:
                TableConstraint::Unique {
                    using_index: Some(index),
                    ..
                },
            ..
        } => Some(("UNIQUE", index.as_str())),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::pg_query::parse_sql;
    use crate::rules::rejected_statements::rejections;
    use crate::rules::test_helpers::{catalog_from_sql, normalized};

    const CUSTOMERS_AND_ORDERS: &str = "
        CREATE TABLE customers (id bigint NOT NULL);
        CREATE TABLE orders (id bigint NOT NULL);";

    const SHARED_IDX_CHANGING_HANDS: &str = "
        ALTER TABLE orders ADD UNIQUE USING INDEX shared_idx;
        CREATE UNIQUE INDEX shared_idx ON customers (id);
        ALTER TABLE orders ADD UNIQUE USING INDEX shared_idx;
        DROP INDEX shared_idx;
        CREATE UNIQUE INDEX shared_idx ON orders (id);
        ALTER TABLE orders ADD UNIQUE USING INDEX shared_idx;";

    fn summary(using_index: Option<&UsingIndex>) -> String {
        match using_index {
            None => "-".to_string(),
            Some(UsingIndex::Missing) => "Missing".to_string(),
            Some(UsingIndex::OfAnotherTable { owner }) => format!("OfAnotherTable({owner})"),
            Some(UsingIndex::OfTable { index, table }) => {
                format!("OfTable({}.{})", table.display_name, index.name)
            }
        }
    }

    #[test]
    fn each_statement_sees_the_index_as_the_earlier_statements_of_the_unit_leave_it() {
        let before = catalog_from_sql(CUSTOMERS_AND_ORDERS);
        let statements = normalized(parse_sql(SHARED_IDX_CHANGING_HANDS));

        let seen: Vec<String> = using_indexes_by_statement(&before, &statements)
            .iter()
            .map(|using_indexes| summary(using_indexes.get("shared_idx")))
            .collect();
        assert_eq!(
            seen,
            [
                "Missing",
                "-",
                "OfAnotherTable(customers)",
                "-",
                "-",
                "OfTable(orders.shared_idx)",
            ]
        );
    }

    #[test]
    fn pgm027_rejects_exactly_the_using_index_statements_that_see_another_tables_index() {
        let before = catalog_from_sql(CUSTOMERS_AND_ORDERS);
        let statements = normalized(parse_sql(SHARED_IDX_CHANGING_HANDS));

        let rejected: Vec<bool> = rejections(&before, &statements)
            .iter()
            .map(Option::is_some)
            .collect();
        let of_another_table: Vec<bool> = using_indexes_by_statement(&before, &statements)
            .iter()
            .map(|using_indexes| {
                matches!(
                    using_indexes.get("shared_idx"),
                    Some(UsingIndex::OfAnotherTable { .. })
                )
            })
            .collect();
        assert_eq!(rejected, of_another_table);
        assert_eq!(rejected, [false, false, true, false, false, false]);
    }
}
