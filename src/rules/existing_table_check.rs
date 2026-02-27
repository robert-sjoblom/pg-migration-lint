//! Shared helper for rules that flag statements targeting pre-existing tables.
//!
//! Used by PGM201, PGM203, PGM301, PGM302, PGM303, and PGM505, which all follow the same
//! pattern: iterate statements, extract a table name from a specific IR variant, check
//! `is_existing_table`, and emit a finding.

use crate::parser::ir::{IrNode, Located, QualifiedName};
use crate::rules::{Finding, LintContext, Rule};

/// Iterate statements, calling `extract` on each `IrNode`. When the closure returns
/// `Some((table_name, message))` and the table is pre-existing, a finding is emitted.
///
/// The closure should pattern-match the specific `IrNode` variant it cares about and
/// return `None` for all other variants.
pub fn check_existing_table(
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
    rule: impl Rule,
    mut extract: impl FnMut(&IrNode) -> Option<(&QualifiedName, String)>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for stmt in statements {
        if let Some((table_name, message)) = extract(&stmt.node)
            && ctx.is_existing_table(table_name.catalog_key())
        {
            findings.push(rule.make_finding(message, ctx.file, &stmt.span));
        }
    }
    findings
}
