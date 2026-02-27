//! Shared helper for rules that inspect `DROP COLUMN` effects on constraints/indexes.
//!
//! Used by PGM010, PGM011, and PGM012, which all follow the same pattern: filter to
//! `ALTER TABLE ... DROP COLUMN` on pre-existing tables, look up the table in
//! `catalog_before`, then inspect constraints involving the dropped column.

use crate::catalog::types::TableState;
use crate::parser::ir::{AlterTable, AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, TableScope, alter_table_check};

/// Iterate `ALTER TABLE ... DROP COLUMN` actions on pre-existing tables. For each one,
/// look up the table in `catalog_before` and call `check` with the column name, ALTER TABLE
/// node, table state, statement wrapper, and lint context.
///
/// Returns all findings collected from the callback.
pub fn check_drop_column_constraints<F>(
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
    mut check: F,
) -> Vec<Finding>
where
    F: FnMut(&str, &AlterTable, &TableState, &Located<IrNode>, &LintContext<'_>) -> Vec<Finding>,
{
    alter_table_check::check_alter_actions(
        statements,
        ctx,
        TableScope::AnyPreExisting,
        |at, action, stmt, ctx| {
            let AlterTableAction::DropColumn { name } = action else {
                return vec![];
            };

            let table_key = at.name.catalog_key();
            let Some(table) = ctx.catalog_before.get_table(table_key) else {
                return vec![];
            };

            check(name, at, table, stmt, ctx)
        },
    )
}
