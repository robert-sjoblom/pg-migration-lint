mod common;

#[test]
fn test_catalog_ops_pgm503_not_triggered_after_drop_not_null() {
    // V001 creates `settings` with UNIQUE NOT NULL on `key` but no PK.
    // V002 does ALTER TABLE settings ALTER COLUMN key DROP NOT NULL.
    // After replay, `key` is nullable → has_unique_not_null() = false → PGM503 silent.
    //
    // Without proper DROP NOT NULL handling in the catalog, `key` would stay
    // NOT NULL and PGM503 would incorrectly fire.
    let findings =
        common::lint_fixture_rules("catalog-ops", &["V002__catalog_ops.sql"], &["PGM503"]);
    assert!(
        findings.is_empty(),
        "PGM503 should NOT fire for settings: key is now nullable after DROP NOT NULL. \
         Got {} finding(s): {:?}",
        findings.len(),
        findings.iter().map(|f| &f.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_catalog_ops_drop_constraint_removes_fk() {
    // V001 adds fk_customer (FK without covering index) on orders.
    // V002 drops it with DROP CONSTRAINT fk_customer.
    // PGM501 collects FKs from IR in the changed file. Since V002 has no
    // ADD CONSTRAINT FK, PGM501 has nothing to flag.
    let findings =
        common::lint_fixture_rules("catalog-ops", &["V002__catalog_ops.sql"], &["PGM501"]);
    assert!(
        findings.is_empty(),
        "PGM501 should NOT fire: V002 adds no FK, and the baseline FK was dropped. \
         Got {} finding(s): {:?}",
        findings.len(),
        findings.iter().map(|f| &f.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_catalog_ops_pipeline_runs_cleanly() {
    // Verify the full pipeline doesn't panic on DROP CONSTRAINT,
    // VALIDATE CONSTRAINT, and DROP NOT NULL.
    let findings = common::lint_fixture("catalog-ops", &["V002__catalog_ops.sql"]);
    // We don't assert exact findings — just that the pipeline completes.
    // No PGM503 for settings (key is now nullable), no PGM501 (no FK added in V002).
    let rule_ids: Vec<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();
    assert!(
        !rule_ids.contains(&"PGM503"),
        "PGM503 should not fire for settings after DROP NOT NULL"
    );
}

#[test]
fn test_catalog_ops_set_drop_default_and_hash_index() {
    // V003 exercises SET DEFAULT, DROP DEFAULT, re-adds an FK, and creates a hash index.
    // Expected findings:
    //   - PGM001: CREATE INDEX without CONCURRENTLY on existing table
    //   - PGM501: FK without covering btree index (hash index does NOT count)
    let findings = common::lint_fixture("catalog-ops", &["V003__defaults_and_hash.sql"]);
    let rule_ids: Vec<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();
    assert!(
        rule_ids.contains(&"PGM001"),
        "PGM001 should fire for CREATE INDEX without CONCURRENTLY on existing table"
    );
    assert!(
        rule_ids.contains(&"PGM501"),
        "PGM501 should fire: hash index does NOT satisfy FK coverage. Got: {:?}",
        rule_ids
    );
}

#[test]
fn test_catalog_ops_hash_index_does_not_suppress_pgm501() {
    // Isolated PGM501 check: V003 adds FK + hash index. The hash index should
    // NOT count as a covering index, so PGM501 must fire.
    let findings =
        common::lint_fixture_rules("catalog-ops", &["V003__defaults_and_hash.sql"], &["PGM501"]);
    assert_eq!(
        findings.len(),
        1,
        "PGM501 should fire exactly once for the FK with only a hash index. Got: {:?}",
        findings.iter().map(|f| &f.message).collect::<Vec<_>>()
    );
}

#[test]
fn test_catalog_ops_set_default_volatile_fires_pgm006() {
    // V004 does ALTER TABLE orders ALTER COLUMN score SET DEFAULT random().
    // PGM006 should fire at INFO level for volatile SET DEFAULT.
    let findings = common::lint_fixture_rules(
        "catalog-ops",
        &["V004__set_default_volatile.sql"],
        &["PGM006"],
    );
    assert_eq!(
        findings.len(),
        1,
        "PGM006 should fire for SET DEFAULT random(). Got: {:?}",
        findings.iter().map(|f| &f.message).collect::<Vec<_>>()
    );
    assert_eq!(
        findings[0].severity.to_string(),
        "INFO",
        "SET DEFAULT volatile should be INFO, not WARNING"
    );
    assert!(
        findings[0].message.contains("NOT backfilled"),
        "Message should mention existing rows are NOT backfilled"
    );
}
