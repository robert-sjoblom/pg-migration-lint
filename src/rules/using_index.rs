//! What the index named in `ALTER TABLE table ADD PRIMARY KEY USING INDEX
//! index` or `ADD UNIQUE USING INDEX index` refers to.
//!
//! PostgreSQL looks the name up in the altered table's schema when the
//! statement runs, so an index created, dropped or renamed earlier in the
//! same migration unit counts. Attaching the index renames it to the
//! constraint's name, so `catalog_after` may no longer have it under the name
//! the statement uses. Each statement is therefore judged against
//! `catalog_before` with the unit's earlier statements replayed on top
//! ([`map_with_state_before`](crate::catalog::replay::map_with_state_before)).
//! [`UsingIndex::in_catalog`] tells an index of another table with
//! [`Catalog::other_table_owning_index`], as PGM027 and replay do, so PGM016,
//! PGM017 and PGM107 never see an index of the altered table where PGM027
//! reports another table's and replay leaves the statement out. When PGM027
//! does not judge the unit ([`unit_is_judged`]), they report the statement
//! as if PostgreSQL accepted it ([`UsingIndex::for_rule_deferring_to_pgm027`]).

use crate::catalog::Catalog;
use crate::catalog::types::{IndexState, TableState};
use crate::names::RelationName;
use crate::parser::ir::IrNode;
use crate::rules::LintContext;
use crate::rules::rejected_statements::unit_is_judged;

/// What the index name in `ALTER TABLE ... ADD ... USING INDEX` refers to in
/// a catalog, borrowed from that catalog.
#[derive(Debug, Clone, Copy)]
pub(super) enum UsingIndex<'a> {
    /// An index of the altered table, with the table as the catalog holds it.
    OfTable {
        index: &'a IndexState,
        table: &'a TableState,
    },
    /// An index of another table in the altered table's schema. PostgreSQL
    /// rejects the statement, which PGM027 reports. `owner` is that table's
    /// display name.
    OfAnotherTable { owner: &'a str },
    /// No index of that name in the altered table's schema, as far as the
    /// catalog knows.
    Missing,
}

impl<'a> UsingIndex<'a> {
    /// What `index_name` refers to in `ALTER TABLE table ADD ... USING INDEX
    /// index_name` against `catalog`. PostgreSQL looks the name up in
    /// `table`'s schema. The altered table need not be in the catalog for an
    /// index of another table: whichever table it is, it is not the index's.
    pub(super) fn in_catalog(catalog: &'a Catalog, table: &RelationName, index_name: &str) -> Self {
        if let Some(owner) = catalog.other_table_owning_index(table, index_name) {
            return Self::OfAnotherTable {
                owner: &owner.display_name,
            };
        }
        Self::taken_for_the_tables_own(catalog, table, index_name)
    }

    /// What `index_name` refers to for PGM016, PGM017 and PGM107, which leave
    /// a statement PostgreSQL rejects to PGM027: [`UsingIndex::in_catalog`]
    /// while PGM027 judges the unit of `ctx` ([`unit_is_judged`]). While it
    /// does not, they report the statement as if PostgreSQL accepted it, which
    /// it does only for an index of the altered table, so an index of another
    /// table counts as the altered table's own.
    pub(super) fn for_rule_deferring_to_pgm027(
        ctx: &LintContext<'_>,
        catalog: &'a Catalog,
        table: &RelationName,
        index_name: &str,
    ) -> Self {
        if unit_is_judged(ctx) {
            Self::in_catalog(catalog, table, index_name)
        } else {
            Self::taken_for_the_tables_own(catalog, table, index_name)
        }
    }

    /// The index `index_name` in `table`'s schema, whichever table the
    /// catalog gives it to, as an index of `table`.
    fn taken_for_the_tables_own(
        catalog: &'a Catalog,
        table: &RelationName,
        index_name: &str,
    ) -> Self {
        match (
            catalog.get_index(&table.in_same_schema(index_name)),
            catalog.get_table(table),
        ) {
            (Some(index), Some(table)) => Self::OfTable { index, table },
            _ => Self::Missing,
        }
    }
}

/// True for an `ALTER TABLE` with a `USING INDEX` constraint.
pub(super) fn has_using_index(node: &IrNode) -> bool {
    matches!(node, IrNode::AlterTable(at) if at.using_index_constraints().next().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::replay::map_with_state_before_if_needed;
    use crate::parser::ir::Located;
    use crate::parser::pg_query::parse_sql;
    use crate::rules::rejected_statements::rejections;
    use crate::rules::test_helpers::{catalog_from_sql, normalized};

    const CUSTOMERS_AND_ORDERS: &str = "
        CREATE TABLE customers (id bigint NOT NULL);
        CREATE TABLE orders (id bigint NOT NULL);";

    const SHARED_IDX_CHANGING_HANDS: &str = "
        ALTER TABLE orders ADD UNIQUE USING INDEX shared_idx;
        ALTER TABLE orders DROP CONSTRAINT shared_idx;
        CREATE UNIQUE INDEX shared_idx ON customers (id);
        ALTER TABLE orders ADD UNIQUE USING INDEX shared_idx;
        DROP INDEX shared_idx;
        CREATE UNIQUE INDEX shared_idx ON orders (id);
        ALTER TABLE orders ADD UNIQUE USING INDEX shared_idx;";

    fn judge_shared_idx_per_statement<T>(
        before: &Catalog,
        statements: &[Located<IrNode>],
        judge: impl Fn(Option<UsingIndex<'_>>) -> T,
    ) -> Vec<T> {
        map_with_state_before_if_needed(before, statements, has_using_index, |state, stmt| {
            let shared_idx = match &stmt.node {
                IrNode::AlterTable(at) => at
                    .using_index_constraints()
                    .find(|(_, index_name)| *index_name == "shared_idx")
                    .map(|(_, index_name)| {
                        UsingIndex::in_catalog(state, at.name.relation(), index_name)
                    }),
                _ => None,
            };
            judge(shared_idx)
        })
    }

    fn summary(using_index: Option<UsingIndex<'_>>) -> String {
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

        let seen: Vec<String> = judge_shared_idx_per_statement(&before, &statements, summary);
        assert_eq!(
            seen,
            [
                "Missing",
                "-",
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
        let of_another_table: Vec<bool> =
            judge_shared_idx_per_statement(&before, &statements, |using_index| {
                matches!(using_index, Some(UsingIndex::OfAnotherTable { .. }))
            });
        assert_eq!(rejected, of_another_table);
        assert_eq!(rejected, [false, false, false, true, false, false, false]);
    }
}
