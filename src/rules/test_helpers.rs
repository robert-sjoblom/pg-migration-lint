//! Shared test helpers for rule unit tests.

use crate::catalog::Catalog;
use crate::catalog::builder::TEST_DEFAULT_SCHEMA;
pub use crate::catalog::builder::{rel, rel_in};
use crate::input::MigrationUnit;
use crate::names::RelationName;
use crate::normalize::resolve_node;
use crate::parser::ir::*;
use crate::rules::LintContext;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Build a `LintContext` with default settings (in transaction, not a down migration).
pub fn make_ctx<'a>(
    before: &'a Catalog,
    after: &'a Catalog,
    file: &'a Path,
    created: &'a HashSet<RelationName>,
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
    created: &'a HashSet<RelationName>,
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
/// // With tables created in the same change, named like `CatalogBuilder`
/// // names them (`"orders"` is `public.orders`):
/// lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["orders"]);
///
/// // With explicit run_in_transaction flag:
/// lint_ctx!(ctx, &before, &after, "migrations/001.sql", txn: false);
/// ```
macro_rules! lint_ctx {
    ($ctx:ident, $before:expr, $after:expr, $file:expr) => {
        let __lint_file = ::std::path::PathBuf::from($file);
        let __lint_created = ::std::collections::HashSet::<$crate::names::RelationName>::new();
        let $ctx = $crate::rules::test_helpers::make_ctx(
            $before, $after, &__lint_file, &__lint_created,
        );
    };
    ($ctx:ident, $before:expr, $after:expr, $file:expr, created: [$($table:expr),+ $(,)?]) => {
        let __lint_file = ::std::path::PathBuf::from($file);
        let __lint_created: ::std::collections::HashSet<$crate::names::RelationName> =
            [$($crate::catalog::builder::test_relation($table)),+].into_iter().collect();
        let $ctx = $crate::rules::test_helpers::make_ctx(
            $before, $after, &__lint_file, &__lint_created,
        );
    };
    ($ctx:ident, $before:expr, $after:expr, $file:expr, txn: $txn:expr) => {
        let __lint_file = ::std::path::PathBuf::from($file);
        let __lint_created = ::std::collections::HashSet::<$crate::names::RelationName>::new();
        let $ctx = $crate::rules::test_helpers::make_ctx_with_txn(
            $before, $after, &__lint_file, &__lint_created, $txn,
        );
    };
}

pub(crate) use lint_ctx;

/// Wrap a parser-form `IrNode` in a `Located` with a dummy span at line 1,
/// resolved with `public` as the default schema, as the pipeline would.
pub fn located(node: IrNode<QualifiedName>) -> Located<IrNode> {
    located_at(node, 1)
}

/// Wrap a parser-form `IrNode` in a `Located` with a dummy span at the given
/// line, resolved with `public` as the default schema, as the pipeline would.
pub fn located_at(node: IrNode<QualifiedName>, line: usize) -> Located<IrNode> {
    Located {
        node: resolve_node(node, TEST_DEFAULT_SCHEMA),
        span: SourceSpan::at(line, line),
    }
}

/// A statement a test hands to [`normalized`]: parser output, which gets
/// resolved, or the output of [`located`], which already is.
pub trait TestStatement {
    /// The statement in resolved form, with `public` as the default schema.
    fn into_resolved(self) -> Located<IrNode>;
}

impl TestStatement for Located<IrNode<QualifiedName>> {
    fn into_resolved(self) -> Located<IrNode> {
        Located {
            node: resolve_node(self.node, TEST_DEFAULT_SCHEMA),
            span: self.span,
        }
    }
}

impl TestStatement for Located<IrNode> {
    fn into_resolved(self) -> Located<IrNode> {
        self
    }
}

/// Run statements through the pipeline's name resolution with `public` as
/// the default schema, so unqualified names display without it.
pub fn normalized<S: TestStatement>(statements: Vec<S>) -> Vec<Located<IrNode>> {
    statements
        .into_iter()
        .map(TestStatement::into_resolved)
        .collect()
}

/// Build the catalog the pipeline produces for `sql`: parsed, resolved with
/// `public` as the default schema, then replayed into an empty catalog.
pub fn catalog_from_sql(sql: &str) -> Catalog {
    let unit = MigrationUnit {
        id: "normalized".to_string(),
        statements: normalized(crate::parser::pg_query::parse_sql(sql)),
        source_file: PathBuf::new(),
        source_line_offset: 1,
        run_in_transaction: true,
        is_down: false,
    };
    let mut catalog = Catalog::new();
    crate::catalog::replay::apply(&mut catalog, &unit);
    catalog
}
