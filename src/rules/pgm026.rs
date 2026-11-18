#![doc = include_str!("docs/pgm026.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::partition_parent_lock::{LockedParent, ParentLocks};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "One transaction locks several partitioned parent tables";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm026.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Critical;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let parent_locks = ParentLocks::of_unit(statements);
    ctx.transaction_windows(statements)
        .iter()
        .filter_map(|window| {
            let mut locks = window
                .statements_in(statements)
                .iter()
                .filter_map(|stmt| Some((stmt, parent_locks.taken_by(&stmt.node, ctx)?)));
            let (first_locking_statement, first_lock) = locks.next()?;
            let mut parents = vec![first_lock.parent];
            for (_, lock) in locks {
                if !parents.iter().any(|parent| parent.key == lock.parent.key) {
                    parents.push(lock.parent);
                }
            }
            (parents.len() >= 2).then(|| {
                rule.make_finding(
                    several_parents_message(&parents),
                    ctx.file,
                    &first_locking_statement.span,
                )
            })
        })
        .collect()
}

fn several_parents_message(parents: &[LockedParent]) -> String {
    let names = parents
        .iter()
        .map(|parent| format!("'{}'", parent.display_name))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "One transaction takes ACCESS EXCLUSIVE on {count} partitioned parents ({names}) and \
         holds each lock until it ends. While it waits for the next parent's lock, all reads \
         and writes on the parents it already locked are blocked, and if a lock times out, \
         the work on every parent rolls back. Change one parent per transaction, for example \
         one migration per parent.",
        count = parents.len(),
    )
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::parser::pg_query::parse_sql;
    use crate::rules::test_helpers::{lint_ctx, located_at, normalized};
    use crate::rules::{Rule, RuleId};

    fn measurements_and_readings_partitioned() -> Catalog {
        CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .partitioned_by(PartitionStrategy::Range, &["id"]);
            })
            .table("measurements_2023", |t| {
                t.column("id", "bigint", false).partition_of("measurements");
            })
            .table("measurements_2024", |t| {
                t.column("id", "bigint", false).partition_of("measurements");
            })
            .table("readings", |t| {
                t.column("id", "bigint", false)
                    .partitioned_by(PartitionStrategy::Range, &["id"]);
            })
            .table("readings_2023", |t| {
                t.column("id", "bigint", false).partition_of("readings");
            })
            .build()
    }

    fn drop_table(name: &str, line: usize) -> Located<IrNode> {
        located_at(
            IrNode::DropTable(DropTable::test(QualifiedName::unqualified(name))),
            line,
        )
    }

    fn create_partition_of(child: &str, parent: &str, line: usize) -> Located<IrNode> {
        located_at(
            IrNode::CreateTable(
                CreateTable::test(QualifiedName::unqualified(child))
                    .with_columns(vec![ColumnDef::test("id", "bigint").with_nullable(false)])
                    .with_partition_of(QualifiedName::unqualified(parent)),
            ),
            line,
        )
    }

    fn alter_parent(
        parent: &str,
        action: AlterTableAction<QualifiedName>,
    ) -> IrNode<QualifiedName> {
        IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(parent),
            actions: vec![action],
        })
    }

    fn attach_partition(parent: &str, child: &str, line: usize) -> Located<IrNode> {
        located_at(
            alter_parent(
                parent,
                AlterTableAction::AttachPartition {
                    child: QualifiedName::unqualified(child),
                },
            ),
            line,
        )
    }

    fn detach(parent: &str, child: &str, concurrent: bool, line: usize) -> Located<IrNode> {
        located_at(
            alter_parent(
                parent,
                AlterTableAction::DetachPartition {
                    child: QualifiedName::unqualified(child),
                    concurrent,
                },
            ),
            line,
        )
    }

    fn detach_partition(parent: &str, child: &str, line: usize) -> Located<IrNode> {
        detach(parent, child, false, line)
    }

    fn detach_partition_concurrently(parent: &str, child: &str, line: usize) -> Located<IrNode> {
        detach(parent, child, true, line)
    }

    fn begin(line: usize) -> Located<IrNode> {
        located_at(IrNode::TransactionControl(TransactionControl::Begin), line)
    }

    fn commit(line: usize) -> Located<IrNode> {
        located_at(IrNode::TransactionControl(TransactionControl::Commit), line)
    }

    #[test]
    fn fires_once_at_the_first_parent_lock_of_a_begin_block_locking_two_parents() {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql", txn: false);

        let stmts = vec![
            begin(1),
            drop_table("measurements_2023", 2),
            create_partition_of("readings_2025", "readings", 3),
            commit(4),
        ];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn fires_when_run_in_transaction_puts_two_parents_in_one_transaction() {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql", txn: true);

        let stmts = vec![
            drop_table("measurements_2023", 1),
            drop_table("readings_2023", 2),
        ];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn fires_on_one_drop_table_of_partitions_of_two_parents_outside_a_transaction_block() {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql", txn: false);

        let stmts = vec![
            drop_table("measurements_2023", 1),
            drop_table("readings_2023", 1),
        ];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn names_each_parent_once_in_the_order_first_locked() {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql");

        let stmts = vec![
            drop_table("readings_2023", 1),
            drop_table("measurements_2023", 2),
            create_partition_of("readings_2025", "readings", 3),
            drop_table("measurements_2024", 4),
        ];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn fires_once_per_transaction_that_locks_two_parents() {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql", txn: false);

        let stmts = vec![
            begin(1),
            drop_table("measurements_2023", 2),
            drop_table("readings_2023", 3),
            commit(4),
            begin(5),
            create_partition_of("measurements_2025", "measurements", 6),
            create_partition_of("readings_2025", "readings", 7),
            commit(8),
        ];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn no_finding_when_one_transaction_locks_one_parent_several_times() {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql");

        let stmts = vec![
            drop_table("measurements_2023", 1),
            drop_table("measurements_2024", 2),
            create_partition_of("measurements_2025", "measurements", 3),
        ];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn no_finding_when_a_drop_and_a_plain_detach_lock_the_same_parent() {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql");

        let stmts = vec![
            drop_table("measurements_2023", 1),
            detach_partition("measurements", "measurements_2024", 2),
        ];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn no_finding_when_each_parent_is_locked_by_its_own_autocommit_statement() {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql", txn: false);

        let stmts = vec![
            drop_table("measurements_2023", 1),
            drop_table("readings_2023", 2),
        ];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn no_finding_when_each_parent_is_locked_in_its_own_begin_block() {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql", txn: false);

        let stmts = vec![
            begin(1),
            drop_table("measurements_2023", 2),
            commit(3),
            begin(4),
            drop_table("readings_2023", 5),
            commit(6),
        ];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn fires_on_plain_detaches_from_two_parents_in_one_transaction() {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql");

        let stmts = vec![
            detach_partition("measurements", "measurements_2023", 1),
            detach_partition("readings", "readings_2023", 2),
        ];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn fires_on_detaching_and_dropping_a_partition_of_each_of_two_parents_in_one_begin_block() {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql", txn: false);

        let stmts = vec![
            begin(1),
            detach_partition("measurements", "measurements_2023", 2),
            drop_table("measurements_2023", 3),
            detach_partition("readings", "readings_2023", 4),
            drop_table("readings_2023", 5),
            commit(6),
        ];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn plain_detach_on_a_second_parent_counts() {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql");

        let stmts = vec![
            drop_table("measurements_2023", 1),
            detach_partition("readings", "readings_2023", 2),
        ];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn no_finding_when_the_transaction_only_attaches_partitions() {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql");

        let stmts = vec![
            attach_partition("measurements", "measurements_2025", 1),
            attach_partition("readings", "readings_2025", 2),
        ];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[rstest]
    #[case::attach(attach_partition("readings", "readings_2025", 2))]
    #[case::concurrent_detach(detach_partition_concurrently("readings", "readings_2023", 2))]
    fn attach_or_concurrent_detach_on_a_second_parent_does_not_count(
        #[case] second_parent_statement: Located<IrNode>,
    ) {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql");

        let stmts = vec![drop_table("measurements_2023", 1), second_parent_statement];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[rstest]
    #[case::create_partition_of(create_partition_of("readings_2025", "readings", 2))]
    #[case::plain_detach(detach_partition("readings", "readings_2023", 2))]
    fn no_finding_when_the_second_parent_is_created_in_the_change(
        #[case] second_parent_statement: Located<IrNode>,
    ) {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql", created: ["readings"]);

        let stmts = vec![drop_table("measurements_2023", 1), second_parent_statement];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[rstest]
    #[case::create_partition_of(create_partition_of("sensors_2025", "sensors", 2))]
    #[case::plain_detach(detach_partition("sensors", "sensors_2023", 2))]
    fn no_finding_when_the_second_parent_is_not_in_the_catalog(
        #[case] second_parent_statement: Located<IrNode>,
    ) {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql");

        let stmts = vec![drop_table("measurements_2023", 1), second_parent_statement];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn plain_detach_from_a_table_that_is_not_partitioned_does_not_count() {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql");

        let stmts = vec![
            drop_table("measurements_2023", 1),
            detach_partition("readings_2023", "readings_2023_h1", 2),
        ];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn dropping_a_child_detached_earlier_in_the_unit_locks_no_parent() {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql", txn: false);

        let stmts = vec![
            detach_partition_concurrently("readings", "readings_2023", 1),
            begin(2),
            drop_table("readings_2023", 3),
            drop_table("measurements_2023", 4),
            commit(5),
        ];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn a_plain_detach_in_its_own_transaction_leaves_the_later_drop_of_its_child_locking_no_parent()
    {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql", txn: false);

        let stmts = vec![
            begin(1),
            detach_partition("readings", "readings_2023", 2),
            commit(3),
            begin(4),
            drop_table("readings_2023", 5),
            drop_table("measurements_2023", 6),
            commit(7),
        ];

        let findings = RuleId::Pgm026.check(&stmts, &ctx);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[rstest]
    #[case::plain(
        "BEGIN; \
         ALTER TABLE measurements DETACH PARTITION measurements_2023; \
         ALTER TABLE readings DETACH PARTITION readings_2023; \
         COMMIT;",
        1
    )]
    #[case::concurrently(
        "BEGIN; \
         ALTER TABLE measurements DETACH PARTITION measurements_2023 CONCURRENTLY; \
         ALTER TABLE readings DETACH PARTITION readings_2023 CONCURRENTLY; \
         COMMIT;",
        0
    )]
    fn parsed_detaches_from_two_parents_in_a_begin_block(
        #[case] sql: &str,
        #[case] expected_findings: usize,
    ) {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql", txn: false);

        let findings = RuleId::Pgm026.check(&normalized(parse_sql(sql)), &ctx);
        assert_eq!(findings.len(), expected_findings, "{findings:?}");
    }

    #[rstest]
    #[case::both_parents_after_the_commit(
        "ALTER TABLE measurements ADD COLUMN note text;\n\
         COMMIT;\n\
         DROP TABLE measurements_2023;\n\
         DROP TABLE readings_2023;",
        &[3]
    )]
    #[case::one_parent_on_each_side_of_the_commit(
        "DROP TABLE measurements_2023;\n\
         COMMIT;\n\
         DROP TABLE readings_2023;",
        &[]
    )]
    #[case::one_parent_on_each_side_of_a_rollback(
        "DROP TABLE measurements_2023;\n\
         ROLLBACK;\n\
         DROP TABLE readings_2023;",
        &[]
    )]
    fn parsed_statements_after_an_end_under_run_in_transaction_are_one_new_transaction(
        #[case] sql: &str,
        #[case] expected_finding_lines: &[usize],
    ) {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql", txn: true);

        let findings = RuleId::Pgm026.check(&normalized(parse_sql(sql)), &ctx);
        let finding_lines: Vec<usize> = findings.iter().map(|f| f.start_line).collect();
        assert_eq!(finding_lines, expected_finding_lines, "{findings:?}");
    }

    #[rstest]
    #[case::one_statement("DROP TABLE measurements_2023, readings_2023;", 1)]
    #[case::two_statements_on_one_line(
        "DROP TABLE measurements_2023; DROP TABLE readings_2023;",
        0
    )]
    fn parsed_drops_outside_a_transaction_block_are_one_transaction_per_statement(
        #[case] sql: &str,
        #[case] expected_findings: usize,
    ) {
        let before = measurements_and_readings_partitioned();
        lint_ctx!(ctx, &before, &before, "migrations/002.sql", txn: false);

        let findings = RuleId::Pgm026.check(&normalized(parse_sql(sql)), &ctx);
        assert_eq!(findings.len(), expected_findings, "{findings:?}");
    }
}
