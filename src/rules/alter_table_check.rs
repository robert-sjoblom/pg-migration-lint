//! Shared helper for rules that inspect ALTER TABLE actions on existing tables.
//!
//! Used by PGM009-PGM015, PGM017, and PGM019, which all follow the same pattern: iterate statements,
//! filter to `AlterTable` on pre-existing tables, then check each action.

use crate::parser::ir::{AlterTable, AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, TableScope};

/// Iterate ALTER TABLE statements targeting pre-existing tables and call `check_action`
/// for each action. Returns all findings collected from the callback.
///
/// The callback receives:
/// - `at`: the AlterTable node (for accessing table name, etc.)
/// - `action`: a single action within the ALTER TABLE
/// - `stmt`: the Located wrapper (for source span)
/// - `ctx`: the lint context
///
/// Return a `Vec<Finding>` to emit findings, or an empty vec to skip.
pub fn check_alter_actions<F>(
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
    scope: TableScope,
    mut check_action: F,
) -> Vec<Finding>
where
    F: FnMut(&AlterTable, &AlterTableAction, &Located<IrNode>, &LintContext<'_>) -> Vec<Finding>,
{
    let mut findings = Vec::new();
    for stmt in statements {
        if let IrNode::AlterTable(ref at) = stmt.node {
            let table_key = at.name.catalog_key();
            if !ctx.table_matches_scope(table_key, scope) {
                continue;
            }
            for action in &at.actions {
                findings.extend(check_action(at, action, stmt, ctx));
            }
        }
    }
    findings
}
