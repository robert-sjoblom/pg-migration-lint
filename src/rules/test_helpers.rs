//! Shared test helpers for rule unit tests.

use crate::catalog::Catalog;
use crate::parser::ir::*;
use crate::rules::LintContext;
use std::collections::HashSet;
use std::path::Path;

/// Build a `LintContext` with default settings (in transaction, not a down migration).
pub fn make_ctx<'a>(
    before: &'a Catalog,
    after: &'a Catalog,
    file: &'a Path,
    created: &'a HashSet<String>,
) -> LintContext<'a> {
    LintContext {
        catalog_before: before,
        catalog_after: after,
        tables_created_in_change: created,
        run_in_transaction: true,
        is_down: false,
        file,
    }
}

/// Build a `LintContext` with an explicit `run_in_transaction` flag.
pub fn make_ctx_with_txn<'a>(
    before: &'a Catalog,
    after: &'a Catalog,
    file: &'a Path,
    created: &'a HashSet<String>,
    run_in_transaction: bool,
) -> LintContext<'a> {
    LintContext {
        catalog_before: before,
        catalog_after: after,
        tables_created_in_change: created,
        run_in_transaction,
        is_down: false,
        file,
    }
}

/// Wrap an `IrNode` in a `Located` with a dummy span at line 1.
pub fn located(node: IrNode) -> Located<IrNode> {
    Located {
        node,
        span: SourceSpan {
            start_line: 1,
            end_line: 1,
            start_offset: 0,
            end_offset: 0,
        },
    }
}
