#![doc = include_str!("docs/pgm003.md")]

use crate::parser::ir::{AlterTableAction, IrNode, Located, TransactionControl};
use crate::rules::rejected_statements::RejectedStatements;
use crate::rules::{
    Finding, LintContext, Rule, Severity, TransactionWindow, TransactionWindowKind,
};

pub(super) const DESCRIPTION: &str = "CONCURRENTLY inside transaction";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm003.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Critical;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let windows = ctx.transaction_windows(statements);
    let rejected = RejectedStatements::new(ctx.catalog_before, statements);

    statements
        .iter()
        .enumerate()
        .filter(|(_, stmt)| runs_concurrently(&stmt.node))
        .filter_map(|(index, stmt)| {
            let message = windows
                .window_of(index)
                .and_then(|window| transaction_block_message(window, statements))?;
            // A DROP INDEX CONCURRENTLY naming a constraint's index fails in or
            // out of a transaction, and PGM027's fix, dropping the constraint
            // instead, needs no CONCURRENTLY.
            (!rejected.contains(&stmt.span))
                .then(|| rule.make_finding(message, ctx.file, &stmt.span))
        })
        .collect()
}

fn runs_concurrently(node: &IrNode) -> bool {
    match node {
        IrNode::CreateIndex(ci) => ci.concurrent,
        IrNode::DropIndex(di) => di.concurrent,
        IrNode::AlterTable(at) => at.actions.iter().any(|action| {
            matches!(
                action,
                AlterTableAction::DetachPartition {
                    concurrent: true,
                    ..
                }
            )
        }),
        IrNode::Reindex(r) => r.concurrent,
        _ => false,
    }
}

/// The message for a `CONCURRENTLY` statement in `window`, saying which
/// transaction it runs in, or `None` when the window is no transaction block.
fn transaction_block_message(
    window: &TransactionWindow,
    statements: &[Located<IrNode>],
) -> Option<String> {
    match window.kind() {
        TransactionWindowKind::Autocommit => None,
        TransactionWindowKind::ToolTransaction => Some(tool_transaction_message(
            end_of_the_previous_tool_transaction(window, statements),
        )),
        TransactionWindowKind::BeginBlock => window
            .statements_in(statements)
            .first()
            .map(|begin| begin_block_message(begin.span.start_line)),
    }
}

fn tool_transaction_message(previous_end: Option<ToolTransactionEnd>) -> String {
    match previous_end {
        None => "CONCURRENTLY cannot run inside a transaction. \
                 Set runInTransaction=\"false\" (Liquibase) or disable \
                 transactions for this migration."
            .to_string(),
        Some(ToolTransactionEnd { keyword, line }) => format!(
            "CONCURRENTLY cannot run inside a transaction. The {keyword} on line {line} \
             ends the migration tool's transaction, but the tool runs the statements \
             after it in a new one. Move this statement to a migration of its own and \
             disable transactions for it (runInTransaction=\"false\" in Liquibase)."
        ),
    }
}

fn begin_block_message(opening_line: usize) -> String {
    format!(
        "CONCURRENTLY cannot run inside a transaction block, and this statement runs in \
         the one the file opens on line {opening_line}. Move it out of the \
         BEGIN ... COMMIT block."
    )
}

/// A `COMMIT` or `ROLLBACK` in the file that ends a tool transaction.
struct ToolTransactionEnd {
    keyword: &'static str,
    line: usize,
}

/// The `COMMIT` or `ROLLBACK` just before `window`, a tool transaction,
/// which ended the tool's previous transaction; `None` when `window` starts
/// the unit.
fn end_of_the_previous_tool_transaction(
    window: &TransactionWindow,
    statements: &[Located<IrNode>],
) -> Option<ToolTransactionEnd> {
    let end = statements.get(window.statement_range().start.checked_sub(1)?)?;
    let keyword = match end.node {
        IrNode::TransactionControl(TransactionControl::Commit) => "COMMIT",
        IrNode::TransactionControl(TransactionControl::Rollback) => "ROLLBACK",
        _ => return None,
    };
    Some(ToolTransactionEnd {
        keyword,
        line: end.span.start_line,
    })
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::catalog::Catalog;
    use crate::parser::ir::*;
    use crate::parser::pg_query::parse_sql;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{catalog_from_sql, lint_ctx, located, normalized};

    const ORDERS: &str = "\
        CREATE TABLE orders (id bigint PRIMARY KEY, code text UNIQUE, status text);
        CREATE INDEX orders_status_idx ON orders (status);";

    fn findings_for_change(change: &str, run_in_transaction: bool) -> Vec<Finding> {
        let before = catalog_from_sql(ORDERS);
        let after = catalog_from_sql(&format!("{ORDERS}\n{change}"));
        lint_ctx!(ctx, &before, &after, "migrations/002.sql", txn: run_in_transaction);
        RuleId::Pgm003.check(&normalized(parse_sql(change)), &ctx)
    }

    fn finding_lines(findings: &[Finding]) -> Vec<usize> {
        findings.iter().map(|finding| finding.start_line).collect()
    }

    #[test]
    fn concurrently_in_a_begin_block_without_run_in_transaction_fires() {
        let change = "BEGIN;
                      CREATE INDEX CONCURRENTLY orders_code_idx ON orders (code);
                      COMMIT;";

        let findings = findings_for_change(change, false);
        insta::assert_yaml_snapshot!(findings);
    }

    #[rstest]
    #[case::create_index("CREATE INDEX CONCURRENTLY orders_code_idx ON orders (code);")]
    #[case::drop_index("DROP INDEX CONCURRENTLY orders_status_idx;")]
    #[case::detach_partition("ALTER TABLE orders DETACH PARTITION orders_2024 CONCURRENTLY;")]
    #[case::reindex("REINDEX TABLE CONCURRENTLY orders;")]
    fn each_concurrently_form_in_a_begin_block_without_run_in_transaction_fires(
        #[case] concurrently: &str,
    ) {
        let change = format!(
            "BEGIN;
             {concurrently}
             COMMIT;"
        );

        let findings = findings_for_change(&change, false);
        assert_eq!(finding_lines(&findings), [2], "{findings:?}");
    }

    #[rstest]
    #[case::start_transaction("START TRANSACTION;", 1)]
    #[case::commit_and_chain(
        "BEGIN;\nALTER TABLE orders ADD COLUMN note text;\nCOMMIT AND CHAIN;",
        3
    )]
    fn begin_block_message_names_the_line_that_opens_the_block(
        #[case] opening: &str,
        #[case] opening_line: usize,
    ) {
        let change =
            format!("{opening}\nCREATE INDEX CONCURRENTLY orders_code_idx ON orders (code);");

        let findings = findings_for_change(&change, false);
        let [finding] = findings.as_slice() else {
            panic!("expected one finding, got {findings:?}");
        };
        assert!(
            finding
                .message
                .contains(&format!("the one the file opens on line {opening_line}.")),
            "{}",
            finding.message
        );
    }

    #[test]
    fn concurrently_after_a_commit_under_run_in_transaction_fires() {
        let change = "ALTER TABLE orders ADD COLUMN note text;
                      COMMIT;
                      CREATE INDEX CONCURRENTLY orders_code_idx ON orders (code);";

        let findings = findings_for_change(change, true);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn concurrently_after_a_rollback_under_run_in_transaction_names_the_rollback() {
        let change = "ALTER TABLE orders ADD COLUMN note text;
                      ROLLBACK;
                      DROP INDEX CONCURRENTLY orders_status_idx;";

        let findings = findings_for_change(change, true);
        let [finding] = findings.as_slice() else {
            panic!("expected one finding, got {findings:?}");
        };
        assert_eq!(finding.start_line, 3);
        assert!(
            finding.message.contains("The ROLLBACK on line 2 ends"),
            "{}",
            finding.message
        );
    }

    #[test]
    fn concurrently_in_a_begin_block_after_a_commit_under_run_in_transaction_fires() {
        let change = "ALTER TABLE orders ADD COLUMN note text;
                      COMMIT;
                      BEGIN;
                      CREATE INDEX CONCURRENTLY orders_code_idx ON orders (code);
                      COMMIT;
                      REINDEX TABLE CONCURRENTLY orders;";

        let findings = findings_for_change(change, true);
        let lines_and_ends: Vec<(usize, bool, bool)> = findings
            .iter()
            .map(|finding| {
                (
                    finding.start_line,
                    finding.message.contains("The COMMIT on line 2 ends"),
                    finding.message.contains("The COMMIT on line 5 ends"),
                )
            })
            .collect();
        assert_eq!(lines_and_ends, [(4, true, false), (6, false, true)]);
    }

    #[test]
    fn begin_block_wrapping_the_unit_under_run_in_transaction_reports_the_tool_transaction() {
        let change = "BEGIN;
                      CREATE INDEX CONCURRENTLY orders_code_idx ON orders (code);
                      COMMIT;";
        let tool_transaction_only = "CREATE INDEX CONCURRENTLY orders_code_idx ON orders (code);";

        let messages = |change: &str| -> Vec<String> {
            findings_for_change(change, true)
                .into_iter()
                .map(|finding| finding.message)
                .collect()
        };
        let wrapped_in_begin = messages(change);
        assert_eq!(wrapped_in_begin.len(), 1, "{wrapped_in_begin:?}");
        assert_eq!(wrapped_in_begin, messages(tool_transaction_only));
    }

    #[rstest]
    #[case::commit("COMMIT;")]
    #[case::rollback("ROLLBACK;")]
    fn concurrently_after_a_begin_block_without_run_in_transaction_no_finding(#[case] end: &str) {
        let change = format!(
            "BEGIN;
             ALTER TABLE orders ADD COLUMN note text;
             {end}
             CREATE INDEX CONCURRENTLY orders_code_idx ON orders (code);"
        );

        let findings = findings_for_change(&change, false);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn concurrently_in_a_second_begin_block_without_run_in_transaction_fires() {
        let change = "BEGIN;
                      ALTER TABLE orders ADD COLUMN note text;
                      COMMIT;
                      BEGIN;
                      CREATE INDEX CONCURRENTLY orders_code_idx ON orders (code);
                      COMMIT;";

        let findings = findings_for_change(change, false);
        assert_eq!(finding_lines(&findings), [5], "{findings:?}");
    }

    #[test]
    fn concurrently_statements_outside_a_begin_block_without_run_in_transaction_no_finding() {
        let change = "ALTER TABLE orders ADD COLUMN note text;
                      CREATE INDEX CONCURRENTLY orders_code_idx ON orders (code);
                      DROP INDEX CONCURRENTLY orders_status_idx;
                      REINDEX TABLE CONCURRENTLY orders;";

        let findings = findings_for_change(change, false);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn only_the_concurrently_statement_inside_the_begin_block_fires() {
        let change = "CREATE INDEX CONCURRENTLY orders_code_idx ON orders (code);
                      BEGIN;
                      CREATE INDEX CONCURRENTLY orders_note_idx ON orders (status, code);
                      COMMIT;
                      DROP INDEX CONCURRENTLY orders_status_idx;";

        let findings = findings_for_change(change, false);
        assert_eq!(finding_lines(&findings), [3], "{findings:?}");
    }

    #[test]
    fn every_concurrently_statement_in_a_tool_transaction_fires() {
        let change = "CREATE INDEX CONCURRENTLY orders_code_idx ON orders (code);
                      DROP INDEX CONCURRENTLY orders_status_idx;
                      ALTER TABLE orders ADD COLUMN note text;
                      REINDEX TABLE CONCURRENTLY orders;";

        let findings = findings_for_change(change, true);
        assert_eq!(finding_lines(&findings), [1, 2, 4], "{findings:?}");
    }

    #[rstest]
    #[case::tool_transaction(true, "DROP INDEX CONCURRENTLY orders_code_key;")]
    #[case::begin_block(false, "BEGIN;\nDROP INDEX CONCURRENTLY orders_code_key;\nCOMMIT;")]
    fn drop_index_concurrently_of_a_constraint_index_in_a_transaction_no_finding(
        #[case] run_in_transaction: bool,
        #[case] change: &str,
    ) {
        let findings = findings_for_change(change, run_in_transaction);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn drop_index_concurrently_of_a_plain_index_beside_a_constraint_index_fires() {
        let change = "BEGIN;
                      DROP INDEX CONCURRENTLY orders_code_key;
                      DROP INDEX CONCURRENTLY orders_status_idx;
                      COMMIT;";

        let findings = findings_for_change(change, false);
        assert_eq!(finding_lines(&findings), [3], "{findings:?}");
    }

    #[test]
    fn test_concurrent_in_transaction_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql", txn: true);

        let stmts = vec![located(IrNode::CreateIndex(
            CreateIndex::test(
                Some("idx_foo".to_string()),
                QualifiedName::unqualified("bar"),
            )
            .with_columns(vec![IndexColumn::Column("col".to_string())])
            .with_concurrent(true),
        ))];

        let findings = RuleId::Pgm003.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_concurrent_no_transaction_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql", txn: false);

        let stmts = vec![located(IrNode::CreateIndex(
            CreateIndex::test(
                Some("idx_foo".to_string()),
                QualifiedName::unqualified("bar"),
            )
            .with_columns(vec![IndexColumn::Column("col".to_string())])
            .with_concurrent(true),
        ))];

        let findings = RuleId::Pgm003.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_concurrent_in_transaction_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql", txn: true);

        let stmts = vec![located(IrNode::CreateIndex(
            CreateIndex::test(
                Some("idx_foo".to_string()),
                QualifiedName::unqualified("bar"),
            )
            .with_columns(vec![IndexColumn::Column("col".to_string())]),
        ))];

        let findings = RuleId::Pgm003.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_index_concurrent_in_transaction_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql", txn: true);

        let stmts = vec![located(IrNode::DropIndex(
            DropIndex::test(QualifiedName::unqualified("idx_foo"))
                .with_concurrent(true)
                .with_if_exists(false),
        ))];

        let findings = RuleId::Pgm003.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_detach_partition_concurrent_in_transaction_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql", txn: true);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("measurements"),
            actions: vec![AlterTableAction::DetachPartition {
                child: QualifiedName::unqualified("measurements_2023"),
                concurrent: true,
            }],
        }))];

        let findings = RuleId::Pgm003.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_detach_partition_no_concurrent_in_transaction_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql", txn: true);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("measurements"),
            actions: vec![AlterTableAction::DetachPartition {
                child: QualifiedName::unqualified("measurements_2023"),
                concurrent: false,
            }],
        }))];

        let findings = RuleId::Pgm003.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_reindex_concurrent_in_transaction_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql", txn: true);

        let stmts = vec![located(IrNode::Reindex(
            Reindex::test_table(QualifiedName::unqualified("orders")).with_concurrent(),
        ))];

        let findings = RuleId::Pgm003.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_reindex_no_concurrent_in_transaction_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql", txn: true);

        let stmts = vec![located(IrNode::Reindex(Reindex::test_table(
            QualifiedName::unqualified("orders"),
        )))];

        let findings = RuleId::Pgm003.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
