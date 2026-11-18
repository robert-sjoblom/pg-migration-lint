use rstest::rstest;

use crate::parser::QualifiedName;
use crate::parser::ir::{IrNode, TransactionControl};
use crate::parser::pg_query::parse_sql;

fn parsed_nodes(sql: &str) -> Vec<IrNode<QualifiedName>> {
    parse_sql(sql)
        .into_iter()
        .map(|located| located.node)
        .collect()
}

#[rstest]
#[case::begin("BEGIN;", TransactionControl::Begin)]
#[case::begin_work("BEGIN WORK;", TransactionControl::Begin)]
#[case::begin_transaction("BEGIN TRANSACTION;", TransactionControl::Begin)]
#[case::begin_isolation_level("BEGIN ISOLATION LEVEL SERIALIZABLE;", TransactionControl::Begin)]
#[case::start_transaction("START TRANSACTION;", TransactionControl::Begin)]
#[case::start_transaction_read_only("START TRANSACTION READ ONLY;", TransactionControl::Begin)]
#[case::commit("COMMIT;", TransactionControl::Commit)]
#[case::commit_work("COMMIT WORK;", TransactionControl::Commit)]
#[case::commit_transaction("COMMIT TRANSACTION;", TransactionControl::Commit)]
#[case::commit_and_no_chain("COMMIT AND NO CHAIN;", TransactionControl::Commit)]
#[case::end("END;", TransactionControl::Commit)]
#[case::end_transaction("END TRANSACTION;", TransactionControl::Commit)]
#[case::rollback("ROLLBACK;", TransactionControl::Rollback)]
#[case::rollback_work("ROLLBACK WORK;", TransactionControl::Rollback)]
#[case::rollback_and_no_chain("ROLLBACK AND NO CHAIN;", TransactionControl::Rollback)]
#[case::abort("ABORT;", TransactionControl::Rollback)]
#[case::abort_transaction("ABORT TRANSACTION;", TransactionControl::Rollback)]
fn transaction_statement_parses_to_its_transaction_control(
    #[case] sql: &str,
    #[case] expected: TransactionControl,
) {
    assert_eq!(
        parsed_nodes(sql),
        [IrNode::TransactionControl(expected)],
        "{sql}"
    );
}

#[rstest]
#[case::commit_and_chain("COMMIT AND CHAIN;", TransactionControl::Commit)]
#[case::end_and_chain("END AND CHAIN;", TransactionControl::Commit)]
#[case::rollback_and_chain("ROLLBACK AND CHAIN;", TransactionControl::Rollback)]
#[case::abort_and_chain("ABORT AND CHAIN;", TransactionControl::Rollback)]
fn chained_end_is_followed_by_the_begin_of_the_next_transaction(
    #[case] sql: &str,
    #[case] ending: TransactionControl,
) {
    assert_eq!(
        parsed_nodes(sql),
        [
            IrNode::TransactionControl(ending),
            IrNode::TransactionControl(TransactionControl::Begin),
        ],
        "{sql}"
    );
}

#[test]
fn chained_end_and_its_begin_share_the_statement_span() {
    let nodes = parse_sql("SELECT 1;\nCOMMIT AND CHAIN;");

    assert_eq!(nodes.len(), 3);
    assert_eq!(nodes[1].span, nodes[2].span);
    assert_eq!(nodes[1].span.start_line, 2);
}

#[test]
fn begin_statement_commit_parse_in_order() {
    let nodes = parsed_nodes("BEGIN;\nCREATE TABLE orders (id bigint);\nCOMMIT;");

    assert!(
        matches!(
            nodes.as_slice(),
            [
                IrNode::TransactionControl(TransactionControl::Begin),
                IrNode::CreateTable(_),
                IrNode::TransactionControl(TransactionControl::Commit),
            ]
        ),
        "{nodes:?}"
    );
}
