//! Transaction windows ([`TransactionWindows`]): a unit's statements grouped
//! by the transaction PostgreSQL runs them in. A rule asks for them through
//! [`LintContext::transaction_windows`](crate::rules::LintContext::transaction_windows)
//! when it cares what one transaction does at once, such as the locks it
//! holds until it ends, or whether a statement runs inside a transaction
//! block, where `CONCURRENTLY` fails.

use std::ops::Range;

use crate::parser::ir::{IrNode, Located, TransactionControl};

/// The transaction windows of one migration unit, in statement order.
///
/// A window is one of:
///
/// - [`Autocommit`](TransactionWindowKind::Autocommit): one SQL statement
///   outside a transaction block, which PostgreSQL runs in a transaction of
///   its own. The IR nodes of one statement share its span (`DROP TABLE a,
///   b` is one `DropTable` per table), and they share a window.
/// - [`BeginBlock`](TransactionWindowKind::BeginBlock): a transaction block
///   the file opens, from its `BEGIN` through the `COMMIT` or `ROLLBACK`
///   that ends it.
/// - [`ToolTransaction`](TransactionWindowKind::ToolTransaction): a
///   transaction the migration tool runs the unit's statements in when
///   `run_in_transaction` is on. The first starts at the unit's first
///   statement, and each `COMMIT` or `ROLLBACK` in the file ends one, after
///   which the next statement starts another. With no `COMMIT` or `ROLLBACK`
///   in the file it is the whole unit.
///
/// Every IR node, `TransactionControl` nodes included, is in exactly one
/// window, and the windows cover the unit in statement order. An empty unit
/// has no windows. The windows follow the transaction state the way
/// PostgreSQL moves it:
///
/// - Inside a transaction block, `BEGIN` does nothing (PostgreSQL warns that
///   a transaction is already in progress) and stays in the open window.
///   `COMMIT` or `ROLLBACK` ends the block and is the last statement of its
///   window. `ROLLBACK` ends a window just as `COMMIT` does, as the
///   statements before it held their locks until it ran.
/// - Outside a transaction block, `BEGIN` opens one and is the first
///   statement of its window. `COMMIT` or `ROLLBACK` does nothing
///   (PostgreSQL warns that there is no transaction in progress) and is an
///   autocommit window of its own.
/// - A block still open at the end of the unit, after a `BEGIN` with no
///   `COMMIT` or `ROLLBACK`, is one window to the end of the unit, as its
///   statements run in one transaction until the unit ends.
/// - With `run_in_transaction`, no statement runs outside a transaction, and
///   every window is a tool transaction. A `COMMIT` or `ROLLBACK` in the
///   file ends the tool's transaction, and the statements after it run in a
///   new one, to the next `COMMIT` or `ROLLBACK` or to the end of the unit.
///   A `COMMIT` as the unit's last statement leaves no statement for a new
///   one, so it opens no window. A `BEGIN` in the file, wherever it is, is a
///   `BEGIN` inside a transaction and does nothing.
///
///   The new transaction after a `COMMIT` is what the runners do:
///   golang-migrate's default mode sends the file as one simple-protocol
///   query, in which PostgreSQL runs the statements after a `COMMIT` in a
///   new implicit transaction block
///   (`verify/lock-tests/tests/pgm003_concurrently_after_commit.rs`), and a
///   JDBC runner with autocommit off (Flyway, Liquibase with
///   `runInTransaction`) sends a fresh `BEGIN` before the next statement,
///   which is not measured. A
///   runner that sends each statement on its own and starts no transaction
///   after the file's `COMMIT`, as `psql --single-transaction` does, runs
///   each of those statements in a transaction of its own instead, while
///   the windows still hold them in one.
/// - `COMMIT AND CHAIN` and `ROLLBACK AND CHAIN` arrive as the end followed
///   by a `Begin` with the same span, so the end closes its window and the
///   `Begin` opens the next. PostgreSQL rejects `AND CHAIN` outside a
///   transaction block (25P01) and starts no transaction. In a script it
///   rejects that way, the windows show an end that does nothing, an
///   autocommit window of its own, followed by a `Begin` that opens a block.
///   With `run_in_transaction`, the `Begin` is the first statement of the
///   next tool transaction and does nothing there: the statements after the
///   end run in a transaction either way.
///
/// Look a statement's window up by its index in the unit
/// ([`window_of`](Self::window_of)), not by its span: the two nodes of an
/// `AND CHAIN` share a span but are in different windows.
///
/// A known limitation: `PREPARE TRANSACTION` ends the transaction block in
/// PostgreSQL, but the IR keeps it `Ignored`, so its window continues past
/// it.
///
/// Windows say nothing about whether a table is new to the change;
/// `LintContext::tables_created_in_change` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionWindows {
    windows: Vec<TransactionWindow>,
}

/// One transaction: the indices of a non-empty, contiguous run of a unit's
/// statements, and how the transaction started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionWindow {
    statement_range: Range<usize>,
    kind: TransactionWindowKind,
}

/// How a [`TransactionWindow`]'s transaction starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionWindowKind {
    /// One SQL statement outside a transaction block, which PostgreSQL runs
    /// in a transaction of its own. A `COMMIT` or `ROLLBACK` outside a
    /// transaction block is one too.
    Autocommit,
    /// A transaction the migration tool runs the unit's statements in when
    /// `run_in_transaction` is on, to the `COMMIT` or `ROLLBACK` in the file
    /// that ends it, or to the end of the unit. The first starts at the
    /// unit's first statement, and each later one at the statement after
    /// such a `COMMIT` or `ROLLBACK`, as JDBC runners with autocommit off and
    /// golang-migrate's default mode start a new transaction there.
    ToolTransaction,
    /// A transaction block a `BEGIN` or `START TRANSACTION` in the file
    /// opens, to the `COMMIT` or `ROLLBACK` that ends it, or to the end of the
    /// unit.
    BeginBlock,
}

/// A transaction block that has started and not yet ended.
#[derive(Debug, Clone, Copy)]
struct OpenBlock {
    start: usize,
    kind: TransactionWindowKind,
}

impl OpenBlock {
    fn ended_before(self, end: usize) -> TransactionWindow {
        TransactionWindow {
            statement_range: self.start..end,
            kind: self.kind,
        }
    }
}

impl TransactionWindows {
    /// Split `statements`, the statements of one unit, into transaction
    /// windows. `run_in_transaction` says whether the migration tool runs
    /// the unit in a transaction, and so starts one before any statement
    /// that runs outside one.
    pub(super) fn new(statements: &[Located<IrNode>], run_in_transaction: bool) -> Self {
        let mut windows: Vec<TransactionWindow> = Vec::new();
        let mut open_block: Option<OpenBlock> = None;
        let mut previous: Option<&Located<IrNode>> = None;

        for (index, stmt) in statements.iter().enumerate() {
            if run_in_transaction && open_block.is_none() {
                open_block = Some(OpenBlock {
                    start: index,
                    kind: TransactionWindowKind::ToolTransaction,
                });
            }
            match (open_block, transaction_control(&stmt.node)) {
                (Some(block), Some(TransactionControl::Commit | TransactionControl::Rollback)) => {
                    windows.push(block.ended_before(index + 1));
                    open_block = None;
                }
                (Some(_), _) => {}
                (None, Some(TransactionControl::Begin)) => {
                    open_block = Some(OpenBlock {
                        start: index,
                        kind: TransactionWindowKind::BeginBlock,
                    });
                }
                (None, _) => match windows.last_mut() {
                    Some(last) if previous.is_some_and(|prev| same_statement(prev, stmt)) => {
                        last.statement_range.end = index + 1;
                    }
                    _ => windows.push(TransactionWindow {
                        statement_range: index..index + 1,
                        kind: TransactionWindowKind::Autocommit,
                    }),
                },
            }
            previous = Some(stmt);
        }

        if let Some(block) = open_block {
            windows.push(block.ended_before(statements.len()));
        }

        Self { windows }
    }

    /// The windows in statement order.
    pub fn iter(&self) -> std::slice::Iter<'_, TransactionWindow> {
        self.windows.iter()
    }

    /// The window holding the statement at `statement_index` in the unit, or
    /// `None` when the unit has no statement there.
    pub fn window_of(&self, statement_index: usize) -> Option<&TransactionWindow> {
        let position = self
            .windows
            .partition_point(|window| window.statement_range.end <= statement_index);
        self.windows
            .get(position)
            .filter(|window| window.statement_range.contains(&statement_index))
    }
}

impl<'w> IntoIterator for &'w TransactionWindows {
    type Item = &'w TransactionWindow;
    type IntoIter = std::slice::Iter<'w, TransactionWindow>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl TransactionWindow {
    /// The indices of the window's statements in the unit.
    pub fn statement_range(&self) -> Range<usize> {
        self.statement_range.clone()
    }

    /// How the window's transaction starts.
    pub fn kind(&self) -> TransactionWindowKind {
        self.kind
    }

    /// True when the window's statements run inside a transaction block,
    /// where PostgreSQL rejects `CONCURRENTLY`.
    pub fn is_transaction_block(&self) -> bool {
        match self.kind {
            TransactionWindowKind::Autocommit => false,
            TransactionWindowKind::ToolTransaction | TransactionWindowKind::BeginBlock => true,
        }
    }

    /// The window's statements, sliced out of `statements`, the statements
    /// of the unit the window was built from.
    pub fn statements_in<'s>(&self, statements: &'s [Located<IrNode>]) -> &'s [Located<IrNode>] {
        statements.get(self.statement_range()).unwrap_or_default()
    }
}

/// The transaction control `node` is, if it is one.
fn transaction_control(node: &IrNode) -> Option<TransactionControl> {
    match node {
        IrNode::TransactionControl(control) => Some(*control),
        _ => None,
    }
}

/// True when `next`, the node after `previous`, comes from the same SQL
/// statement: a statement split into several nodes gives each its span.
/// Transaction control statements never share a window with another node.
fn same_statement(previous: &Located<IrNode>, next: &Located<IrNode>) -> bool {
    previous.span == next.span
        && transaction_control(&previous.node).is_none()
        && transaction_control(&next.node).is_none()
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::TransactionWindowKind::{Autocommit, BeginBlock, ToolTransaction};
    use super::*;
    use crate::parser::pg_query::parse_sql;
    use crate::rules::test_helpers::normalized;

    fn statements(sql: &str) -> Vec<Located<IrNode>> {
        normalized(parse_sql(sql))
    }

    fn window_shapes(
        sql: &str,
        run_in_transaction: bool,
    ) -> Vec<(Range<usize>, TransactionWindowKind)> {
        TransactionWindows::new(&statements(sql), run_in_transaction)
            .iter()
            .map(|window| (window.statement_range(), window.kind()))
            .collect()
    }

    #[rstest]
    fn empty_unit_has_no_windows(#[values(true, false)] run_in_transaction: bool) {
        assert_eq!(window_shapes("", run_in_transaction), []);
    }

    #[test]
    fn without_run_in_transaction_each_statement_is_its_own_autocommit_window() {
        let sql = "CREATE TABLE a (id int);
                   CREATE TABLE b (id int);
                   DROP TABLE c;";

        assert_eq!(
            window_shapes(sql, false),
            [(0..1, Autocommit), (1..2, Autocommit), (2..3, Autocommit)]
        );
    }

    #[test]
    fn nodes_of_one_multi_table_statement_share_an_autocommit_window() {
        let sql = "DROP TABLE a, b;
                   DROP TABLE c;";

        assert_eq!(
            window_shapes(sql, false),
            [(0..2, Autocommit), (2..3, Autocommit)]
        );
    }

    #[test]
    fn begin_commit_block_is_one_window_from_begin_through_commit() {
        let sql = "BEGIN;
                   CREATE TABLE a (id int);
                   CREATE TABLE b (id int);
                   COMMIT;";

        assert_eq!(window_shapes(sql, false), [(0..4, BeginBlock)]);
    }

    #[test]
    fn statements_around_a_begin_block_are_autocommit() {
        let sql = "CREATE TABLE a (id int);
                   BEGIN;
                   CREATE TABLE b (id int);
                   COMMIT;
                   CREATE TABLE c (id int);";

        assert_eq!(
            window_shapes(sql, false),
            [(0..1, Autocommit), (1..4, BeginBlock), (4..5, Autocommit)]
        );
    }

    #[test]
    fn rollback_ends_a_begin_block_like_commit() {
        let sql = "BEGIN;
                   DROP TABLE a;
                   ROLLBACK;
                   DROP TABLE b;";

        assert_eq!(
            window_shapes(sql, false),
            [(0..3, BeginBlock), (3..4, Autocommit)]
        );
    }

    #[test]
    fn consecutive_begin_blocks_are_separate_windows() {
        let sql = "BEGIN;
                   DROP TABLE a;
                   COMMIT;
                   START TRANSACTION;
                   DROP TABLE b;
                   END;";

        assert_eq!(
            window_shapes(sql, false),
            [(0..3, BeginBlock), (3..6, BeginBlock)]
        );
    }

    #[test]
    fn begin_inside_a_begin_block_stays_in_that_block() {
        let sql = "BEGIN;
                   DROP TABLE a;
                   BEGIN;
                   DROP TABLE b;
                   COMMIT;
                   DROP TABLE c;";

        assert_eq!(
            window_shapes(sql, false),
            [(0..5, BeginBlock), (5..6, Autocommit)]
        );
    }

    #[rstest]
    #[case::commit("COMMIT;")]
    #[case::rollback("ROLLBACK;")]
    fn end_outside_a_transaction_is_its_own_autocommit_window(#[case] stray_end: &str) {
        let sql = format!(
            "DROP TABLE a;
             {stray_end}
             DROP TABLE b;"
        );

        assert_eq!(
            window_shapes(&sql, false),
            [(0..1, Autocommit), (1..2, Autocommit), (2..3, Autocommit)]
        );
    }

    #[test]
    fn end_after_a_closed_begin_block_is_its_own_autocommit_window() {
        let sql = "BEGIN;
                   DROP TABLE a;
                   COMMIT;
                   COMMIT;";

        assert_eq!(
            window_shapes(sql, false),
            [(0..3, BeginBlock), (3..4, Autocommit)]
        );
    }

    #[test]
    fn unterminated_begin_block_runs_to_the_end_of_the_unit() {
        let sql = "DROP TABLE a;
                   BEGIN;
                   DROP TABLE b;
                   DROP TABLE c;";

        assert_eq!(
            window_shapes(sql, false),
            [(0..1, Autocommit), (1..4, BeginBlock)]
        );
    }

    #[test]
    fn begin_as_the_last_statement_is_a_window_of_its_own() {
        let sql = "DROP TABLE a;
                   BEGIN;";

        assert_eq!(
            window_shapes(sql, false),
            [(0..1, Autocommit), (1..2, BeginBlock)]
        );
    }

    #[test]
    fn run_in_transaction_puts_the_whole_unit_in_one_window() {
        let sql = "CREATE TABLE a (id int);
                   DROP TABLE b, c;
                   DROP TABLE d;";

        assert_eq!(window_shapes(sql, true), [(0..4, ToolTransaction)]);
    }

    #[test]
    fn begin_and_commit_wrapping_the_unit_under_run_in_transaction_are_one_window() {
        let sql = "BEGIN;
                   DROP TABLE a;
                   DROP TABLE b;
                   COMMIT;";

        assert_eq!(window_shapes(sql, true), [(0..4, ToolTransaction)]);
    }

    #[rstest]
    #[case::commit("COMMIT;")]
    #[case::rollback("ROLLBACK;")]
    fn end_under_run_in_transaction_starts_a_new_tool_transaction_to_the_end_of_the_unit(
        #[case] end: &str,
    ) {
        let sql = format!(
            "DROP TABLE a;
             {end}
             CREATE INDEX CONCURRENTLY idx_b ON b (id);
             DROP TABLE c;"
        );

        assert_eq!(
            window_shapes(&sql, true),
            [(0..2, ToolTransaction), (2..4, ToolTransaction)]
        );
    }

    #[test]
    fn each_end_under_run_in_transaction_starts_the_next_tool_transaction() {
        let sql = "DROP TABLE a;
                   COMMIT;
                   DROP TABLE b;
                   ROLLBACK;
                   DROP TABLE c;
                   COMMIT;
                   DROP TABLE d;";

        assert_eq!(
            window_shapes(sql, true),
            [
                (0..2, ToolTransaction),
                (2..4, ToolTransaction),
                (4..6, ToolTransaction),
                (6..7, ToolTransaction)
            ]
        );
    }

    #[test]
    fn end_as_the_last_statement_under_run_in_transaction_opens_no_window() {
        let sql = "DROP TABLE a;
                   DROP TABLE b;
                   COMMIT;";

        assert_eq!(window_shapes(sql, true), [(0..3, ToolTransaction)]);
    }

    #[test]
    fn consecutive_ends_under_run_in_transaction_each_end_a_tool_transaction() {
        let sql = "DROP TABLE a;
                   COMMIT;
                   COMMIT;";

        assert_eq!(
            window_shapes(sql, true),
            [(0..2, ToolTransaction), (2..3, ToolTransaction)]
        );
    }

    #[test]
    fn begin_after_an_end_under_run_in_transaction_does_nothing_in_the_new_tool_transaction() {
        let sql = "DROP TABLE a;
                   COMMIT;
                   DROP TABLE b;
                   BEGIN;
                   DROP TABLE c;
                   DROP TABLE d;";

        assert_eq!(
            window_shapes(sql, true),
            [(0..2, ToolTransaction), (2..6, ToolTransaction)]
        );
    }

    #[test]
    fn begin_block_right_after_an_end_under_run_in_transaction_is_a_tool_transaction() {
        let sql = "DROP TABLE a;
                   COMMIT;
                   BEGIN;
                   CREATE INDEX CONCURRENTLY idx_b ON b (id);
                   COMMIT;
                   DROP TABLE c;";

        assert_eq!(
            window_shapes(sql, true),
            [
                (0..2, ToolTransaction),
                (2..5, ToolTransaction),
                (5..6, ToolTransaction)
            ]
        );
    }

    #[rstest]
    #[case::commit_and_chain("COMMIT AND CHAIN;")]
    #[case::rollback_and_chain("ROLLBACK AND CHAIN;")]
    fn chained_end_closes_the_begin_block_and_opens_the_next(#[case] chained_end: &str) {
        let sql = format!(
            "BEGIN;
             DROP TABLE a;
             {chained_end}
             DROP TABLE b;
             COMMIT;
             DROP TABLE c;"
        );

        assert_eq!(
            window_shapes(&sql, false),
            [(0..3, BeginBlock), (3..6, BeginBlock), (6..7, Autocommit)]
        );
    }

    #[test]
    fn commit_and_chain_under_run_in_transaction_continues_in_a_new_tool_transaction() {
        let sql = "DROP TABLE a;
                   COMMIT AND CHAIN;
                   CREATE INDEX CONCURRENTLY idx_b ON b (id);";

        assert_eq!(
            window_shapes(sql, true),
            [(0..2, ToolTransaction), (2..4, ToolTransaction)]
        );
    }

    #[test]
    fn commit_and_chain_outside_a_transaction_is_a_stray_commit_then_a_begin() {
        let sql = "DROP TABLE a;
                   COMMIT AND CHAIN;
                   DROP TABLE b;";

        assert_eq!(
            window_shapes(sql, false),
            [(0..1, Autocommit), (1..2, Autocommit), (2..4, BeginBlock)]
        );
    }

    #[test]
    fn only_autocommit_windows_are_outside_a_transaction_block() {
        let sql = "DROP TABLE a;
                   BEGIN;
                   DROP TABLE b;
                   COMMIT;";
        let unit_statements = statements(sql);

        let tool = TransactionWindows::new(&unit_statements, true);
        let file = TransactionWindows::new(&unit_statements, false);

        let blocks = |windows: &TransactionWindows| -> Vec<bool> {
            windows
                .iter()
                .map(TransactionWindow::is_transaction_block)
                .collect()
        };
        assert_eq!(blocks(&tool), [true]);
        assert_eq!(blocks(&file), [false, true]);
    }

    #[test]
    fn window_of_finds_the_window_holding_each_statement() {
        let sql = "DROP TABLE a;
                   BEGIN;
                   DROP TABLE b;
                   COMMIT AND CHAIN;
                   DROP TABLE c;";
        let unit_statements = statements(sql);
        let windows = TransactionWindows::new(&unit_statements, false);

        let ranges: Vec<Option<Range<usize>>> = (0..=unit_statements.len())
            .map(|index| {
                windows
                    .window_of(index)
                    .map(TransactionWindow::statement_range)
            })
            .collect();

        assert_eq!(
            ranges,
            [
                Some(0..1),
                Some(1..4),
                Some(1..4),
                Some(1..4),
                Some(4..6),
                Some(4..6),
                None
            ]
        );
    }

    #[test]
    fn statements_in_slices_the_window_out_of_the_unit() {
        let unit_statements = statements(
            "DROP TABLE a;
             BEGIN;
             DROP TABLE b;
             COMMIT;",
        );
        let windows = TransactionWindows::new(&unit_statements, false);

        let lines: Vec<Vec<usize>> = windows
            .iter()
            .map(|window| {
                window
                    .statements_in(&unit_statements)
                    .iter()
                    .map(|stmt| stmt.span.start_line)
                    .collect()
            })
            .collect();

        assert_eq!(lines, [vec![1], vec![2, 3, 4]]);
    }

    const STATEMENT_KINDS: [&str; 6] = [
        "DROP TABLE t;",
        "DROP TABLE t, u;",
        "BEGIN;",
        "COMMIT;",
        "ROLLBACK;",
        "COMMIT AND CHAIN;",
    ];

    fn every_unit_with_at_most(statement_count: usize) -> Vec<String> {
        let mut units = vec![String::new()];
        let mut units_of_last_length = vec![String::new()];
        for _ in 0..statement_count {
            units_of_last_length = units_of_last_length
                .iter()
                .flat_map(|unit| {
                    STATEMENT_KINDS
                        .iter()
                        .map(move |statement| format!("{unit}{statement}\n"))
                })
                .collect();
            units.extend(units_of_last_length.iter().cloned());
        }
        units
    }

    struct WindowedUnit {
        sql: String,
        statements: Vec<Located<IrNode>>,
        windows: TransactionWindows,
    }

    impl WindowedUnit {
        fn windows_of_kind(
            &self,
            kind: TransactionWindowKind,
        ) -> impl Iterator<Item = &[Located<IrNode>]> {
            self.windows
                .iter()
                .filter(move |window| window.kind() == kind)
                .map(|window| window.statements_in(&self.statements))
        }
    }

    fn every_windowed_unit(run_in_transaction: bool) -> Vec<WindowedUnit> {
        every_unit_with_at_most(4)
            .into_iter()
            .map(|sql| {
                let unit_statements = statements(&sql);
                let windows = TransactionWindows::new(&unit_statements, run_in_transaction);
                WindowedUnit {
                    sql,
                    statements: unit_statements,
                    windows,
                }
            })
            .collect()
    }

    fn is_begin(stmt: &Located<IrNode>) -> bool {
        transaction_control(&stmt.node) == Some(TransactionControl::Begin)
    }

    fn ends_transaction(stmt: &Located<IrNode>) -> bool {
        matches!(
            transaction_control(&stmt.node),
            Some(TransactionControl::Commit | TransactionControl::Rollback)
        )
    }

    #[rstest]
    fn every_statement_is_in_exactly_one_window(#[values(true, false)] run_in_transaction: bool) {
        for unit in every_windowed_unit(run_in_transaction) {
            let sql = &unit.sql;
            let mut next_statement = 0;
            for window in &unit.windows {
                let range = window.statement_range();
                assert_eq!(range.start, next_statement, "{sql}");
                assert!(!range.is_empty(), "{sql}");
                next_statement = range.end;
            }
            assert_eq!(next_statement, unit.statements.len(), "{sql}");

            for index in 0..unit.statements.len() {
                let holding = unit
                    .windows
                    .window_of(index)
                    .map(TransactionWindow::statement_range);
                assert!(
                    holding.is_some_and(|range| range.contains(&index)),
                    "statement {index} of {sql}"
                );
            }
        }
    }

    #[rstest]
    fn every_begin_block_starts_with_begin(#[values(true, false)] run_in_transaction: bool) {
        for unit in every_windowed_unit(run_in_transaction) {
            for block in unit.windows_of_kind(BeginBlock) {
                assert!(block.first().is_some_and(is_begin), "{}", unit.sql);
            }
        }
    }

    #[rstest]
    fn a_block_ends_with_commit_or_rollback_unless_it_runs_to_the_end_of_the_unit(
        #[values(true, false)] run_in_transaction: bool,
    ) {
        for unit in every_windowed_unit(run_in_transaction) {
            let sql = &unit.sql;
            for window in unit.windows.iter().filter(|w| w.is_transaction_block()) {
                let block = window.statements_in(&unit.statements);
                let Some((last, before_last)) = block.split_last() else {
                    panic!("empty block in {sql}");
                };
                assert!(!before_last.iter().any(ends_transaction), "{sql}");
                if window.statement_range().end < unit.statements.len() {
                    assert!(ends_transaction(last), "{sql}");
                }
            }
        }
    }

    #[rstest]
    fn every_autocommit_window_is_one_statement_without_begin(
        #[values(true, false)] run_in_transaction: bool,
    ) {
        for unit in every_windowed_unit(run_in_transaction) {
            for window in unit.windows_of_kind(Autocommit) {
                assert!(
                    window.windows(2).all(|pair| pair[0].span == pair[1].span),
                    "{}",
                    unit.sql
                );
                assert!(!window.iter().any(is_begin), "{}", unit.sql);
            }
        }
    }

    #[rstest]
    fn every_window_is_a_tool_transaction_exactly_when_run_in_transaction_is_on(
        #[values(true, false)] run_in_transaction: bool,
    ) {
        for unit in every_windowed_unit(run_in_transaction) {
            for window in &unit.windows {
                assert_eq!(
                    window.kind() == ToolTransaction,
                    run_in_transaction,
                    "{:?} in {}",
                    window.statement_range(),
                    unit.sql
                );
            }
        }
    }
}
