mod common;

/// Reproduces live observation: a real 5-parent,
/// 20-quarterly-child-to-5-yearly-child repartition inside one BEGIN/COMMIT
/// used to produce 62 findings (PGM201 x20, PGM401 x20, PGM402 x14, PGM501
/// x4, PGM502 x2, PGM508 x2) and not one mentioned locks, the parent, or
/// blocking.
#[test]
fn test_repartition_reports_one_pgm024_finding_per_parent() {
    let findings = common::lint_fixture_rules(
        "partition-lock-hazard",
        &["V002__repartition.sql"],
        &["PGM024"],
    );

    assert_eq!(
        findings.len(),
        5,
        "20 DROP TABLEs + 5 CREATE TABLE ... PARTITION OF statements across 5 distinct parents should dedup to exactly 5 findings, one per parent. Got:\n  {}",
        common::format_findings(&findings)
    );

    for finding in &findings {
        assert_eq!(
            finding.severity,
            pg_migration_lint::Severity::Critical,
            "PGM024 must stay Critical to keep the default fail_on=\"critical\" gate red. Got:\n  {}",
            common::format_findings(&findings)
        );
    }
}

/// End-to-end regression test for the same-unit DETACH-then-DROP guard,
/// run through the real parser and `normalize_schemas` pass rather than
/// hand-built IR, so it can catch a regression in normalization's handling
/// of `AlterTableAction::DetachPartition`'s `child` field.
#[test]
fn test_safe_detach_then_drop_no_finding() {
    let findings = common::lint_fixture_rules(
        "partition-lock-hazard",
        &["V003__safe_detach_then_drop.sql"],
        &["PGM024"],
    );

    assert!(
        findings.is_empty(),
        "DETACH PARTITION CONCURRENTLY then DROP TABLE in the same file is the safe pattern \
         PGM004 recommends — PGM024 must not fire on it. Got:\n  {}",
        common::format_findings(&findings)
    );
}
