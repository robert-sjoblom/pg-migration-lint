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

/// Create a [`LintContext`] with default settings, reducing the 3-line
/// `file` + `created` + `make_ctx` boilerplate to a single invocation.
///
/// The macro creates hygienic bindings for `PathBuf` and `HashSet` that live
/// in the caller's scope (satisfying the borrow lifetimes on `LintContext`).
///
/// ```ignore
/// // Empty created set:
/// lint_ctx!(ctx, &before, &after, "migrations/002.sql");
///
/// // With tables created in the same change:
/// lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["orders"]);
///
/// // With explicit run_in_transaction flag:
/// lint_ctx!(ctx, &before, &after, "migrations/001.sql", txn: false);
/// ```
macro_rules! lint_ctx {
    ($ctx:ident, $before:expr, $after:expr, $file:expr) => {
        let __lint_file = ::std::path::PathBuf::from($file);
        let __lint_created = ::std::collections::HashSet::<String>::new();
        let $ctx = $crate::rules::test_helpers::make_ctx(
            $before, $after, &__lint_file, &__lint_created,
        );
    };
    ($ctx:ident, $before:expr, $after:expr, $file:expr, created: [$($table:expr),+ $(,)?]) => {
        let __lint_file = ::std::path::PathBuf::from($file);
        let __lint_created: ::std::collections::HashSet<String> =
            [$(($table).to_string()),+].into_iter().collect();
        let $ctx = $crate::rules::test_helpers::make_ctx(
            $before, $after, &__lint_file, &__lint_created,
        );
    };
    ($ctx:ident, $before:expr, $after:expr, $file:expr, txn: $txn:expr) => {
        let __lint_file = ::std::path::PathBuf::from($file);
        let __lint_created = ::std::collections::HashSet::<String>::new();
        let $ctx = $crate::rules::test_helpers::make_ctx_with_txn(
            $before, $after, &__lint_file, &__lint_created, $txn,
        );
    };
}

pub(crate) use lint_ctx;

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
