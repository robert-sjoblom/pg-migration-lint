use pg_migration_lint::Finding;

use crate::common::{
    APPLY_SUPPRESSIONS, changed_files_for, format_findings, lint_fixture, lint_fixture_inner,
    lint_fixture_rules, normalize_findings,
};

mod common;

#[test]
fn test_schema_qualified_no_collision() {
    // Lint V002 and V003 as changed; V001 is replayed as baseline.
    // V001 creates myschema.customers and (unqualified) orders.
    // After normalization: myschema.customers stays myschema.customers,
    // orders becomes public.orders. They must be distinct catalog entries.
    //
    // V002 adds FK + covering index (no PGM003).
    // V003 creates index on myschema.customers without CONCURRENTLY -> PGM001.
    // V002's index on orders also fires PGM001 (orders is pre-existing from V001).
    let findings = lint_fixture_rules(
        "schema-qualified",
        &["V002__add_fk_and_index.sql", "V003__alter_schema_table.sql"],
        &["PGM001", "PGM501"],
    );

    let pgm001: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM001")
        .collect();

    // Two PGM001 findings: one for idx_orders_customer_id on public.orders,
    // one for idx_customers_name on myschema.customers.
    assert_eq!(
        pgm001.len(),
        2,
        "Expected 2 PGM001 findings (one per pre-existing table). Got:\n  {}",
        format_findings(&findings)
    );

    // Verify one mentions myschema.customers (explicitly qualified) and the other
    // mentions just 'orders' (unqualified — display_name omits the synthetic public. prefix).
    let mentions_myschema = pgm001
        .iter()
        .any(|f| f.message.contains("myschema.customers"));
    let mentions_orders = pgm001.iter().any(|f| f.message.contains("'orders'"));
    assert!(
        mentions_myschema,
        "Expected a PGM001 finding mentioning 'myschema.customers'. Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        mentions_orders,
        "Expected a PGM001 finding mentioning 'orders' (without synthetic schema prefix). Got:\n  {}",
        format_findings(&findings)
    );

    // PGM501 should NOT fire: the covering index is in V002.
    let pgm501: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM501")
        .collect();
    assert!(
        pgm501.is_empty(),
        "PGM501 should not fire (covering index present). Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_schema_qualified_cross_schema_fk() {
    // Lint only V002. V001 is replayed as history.
    // V002 adds FK on orders.customer_id referencing myschema.customers(id).
    // myschema.customers exists in catalog_before (from V001 replay).
    // The covering index idx_orders_customer_id is added in the same file.
    // Expect no PGM003 finding.
    // V002's CREATE INDEX on pre-existing orders fires PGM001.
    let findings = lint_fixture_rules(
        "schema-qualified",
        &["V002__add_fk_and_index.sql"],
        &["PGM001", "PGM501", "PGM014"],
    );
    let pgm501: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM501")
        .collect();

    assert!(
        pgm501.is_empty(),
        "PGM501 should not fire (covering index in same file). Got:\n  {}",
        format_findings(&findings)
    );

    // PGM001 fires exactly once for CREATE INDEX on pre-existing orders table
    let pgm001: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM001")
        .collect();
    assert_eq!(
        pgm001.len(),
        1,
        "Expected exactly 1 PGM001 finding for CREATE INDEX on pre-existing orders. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM014 fires for the FK without NOT VALID on pre-existing orders table
    let pgm014: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM014")
        .collect();
    assert_eq!(
        pgm014.len(),
        1,
        "Expected exactly 1 PGM014 finding for FK without NOT VALID. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_schema_qualified_pgm001_fires() {
    // Lint only V003. V001 and V002 are replayed as history.
    // myschema.customers exists in catalog_before (from V001).
    // V003 creates index on myschema.customers without CONCURRENTLY -> PGM001.
    let findings = lint_fixture_rules(
        "schema-qualified",
        &["V003__alter_schema_table.sql"],
        &["PGM001"],
    );

    assert_eq!(
        findings.len(),
        1,
        "Expected exactly 1 PGM001 finding. Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        findings[0].message.contains("myschema.customers"),
        "PGM001 message should mention 'myschema.customers'. Got: {}",
        findings[0].message
    );
    assert!(
        findings[0].message.contains("CONCURRENTLY"),
        "PGM001 message should mention CONCURRENTLY. Got: {}",
        findings[0].message
    );
}

#[test]
fn test_schema_qualified_custom_default_schema() {
    // Use default_schema = "myschema" instead of "public".
    // With this setting:
    //   - V001's unqualified `orders` normalizes to `myschema.orders`
    //   - V001's `myschema.customers` stays `myschema.customers`
    //   - V002's CREATE INDEX on `orders` targets `myschema.orders`
    //   - V003's CREATE INDEX on `myschema.customers` targets `myschema.customers`
    //
    // Lint V002 and V003 as changed; V001 is replayed as baseline.
    // Both tables are pre-existing (from V001 replay), so PGM001 fires
    // for both indexes.
    let findings = lint_fixture_inner(
        "schema-qualified",
        &["V002__add_fk_and_index.sql", "V003__alter_schema_table.sql"],
        "myschema",
        &["PGM001", "PGM501"],
        &[],
        APPLY_SUPPRESSIONS,
    );

    // PGM001 should fire for the myschema.customers index (V003)
    let pgm001: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM001")
        .collect();
    assert!(
        pgm001
            .iter()
            .any(|f| f.message.contains("myschema.customers")),
        "Expected PGM001 for myschema.customers index. Got:\n  {}",
        format_findings(&findings)
    );

    // No PGM501 (covering index exists for the FK)
    let pgm501: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM501")
        .collect();
    assert!(
        pgm501.is_empty(),
        "PGM501 should not fire (covering index present). Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_multi_schema_all_findings() {
    let changed = changed_files_for("multi-schema");
    let findings = lint_fixture("multi-schema", &changed);
    let findings = normalize_findings(findings, "multi-schema");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_multi_schema_cross_schema_fks() {
    let findings = lint_fixture("multi-schema", &["V002__cross_schema_fks.sql"]);
    let findings = normalize_findings(findings, "multi-schema");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_multi_schema_same_name_isolation() {
    let findings = lint_fixture("multi-schema", &["V003__same_name_isolation.sql"]);
    let findings = normalize_findings(findings, "multi-schema");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_multi_schema_drop_isolation() {
    let findings = lint_fixture("multi-schema", &["V004__drop_schema_isolation.sql"]);
    let findings = normalize_findings(findings, "multi-schema");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_multi_schema_custom_default_schema() {
    // With default_schema="inventory", unqualified `orders` normalizes to
    // inventory.orders and V004's `CREATE TABLE users` becomes inventory.users.
    let changed = changed_files_for("multi-schema");
    let findings = lint_fixture_inner(
        "multi-schema",
        &changed,
        "inventory",
        &[],
        &[],
        APPLY_SUPPRESSIONS,
    );
    let findings = normalize_findings(findings, "multi-schema");
    insta::assert_yaml_snapshot!(findings);
}
