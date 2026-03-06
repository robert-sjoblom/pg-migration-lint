mod common;

#[cfg(feature = "bridge-tests")]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use pg_migration_lint::input::RawMigrationUnit;
    use pg_migration_lint::input::liquibase_bridge::{BridgeLoader, resolve_source_paths};
    use pg_migration_lint::input::liquibase_updatesql::UpdateSqlLoader;
    use pg_migration_lint::suppress::parse_suppressions;
    use pg_migration_lint::{Finding, LintPipeline, RuleId};

    use super::common;

    fn bridge_jar_path() -> PathBuf {
        if let Ok(path) = std::env::var("BRIDGE_JAR_PATH") {
            PathBuf::from(path)
        } else {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("bridge/target/liquibase-bridge-1.0.0.jar")
        }
    }

    /// Shared replay+lint logic for Liquibase loaders (bridge and update-sql).
    ///
    /// Converts raw migration units into `MigrationUnit`s, replays catalog
    /// history, and runs the full rule engine on changed units.
    /// The only difference between bridge and update-sql is HOW they produce
    /// `raw_units`; everything after that is identical.
    fn lint_loaded_units(raw_units: Vec<RawMigrationUnit>, changed_ids: &[&str]) -> Vec<Finding> {
        let units: Vec<pg_migration_lint::input::MigrationUnit> = raw_units
            .into_iter()
            .map(|r| r.into_migration_unit("public"))
            .collect();

        let changed_set: HashSet<String> = changed_ids.iter().map(|s| s.to_string()).collect();

        let all_rules: Vec<RuleId> = RuleId::lint_rules().collect();

        let mut pipeline = LintPipeline::new();
        let mut all_findings: Vec<Finding> = Vec::new();

        for unit in &units {
            let is_changed = changed_set.is_empty() || changed_set.contains(&unit.id);

            if is_changed {
                let mut unit_findings = pipeline.lint(unit, &all_rules);

                let source = std::fs::read_to_string(&unit.source_file).unwrap_or_default();
                let suppressions = parse_suppressions(&source);
                unit_findings.retain(|f| !suppressions.is_suppressed(f.rule_id, f.start_line));

                all_findings.extend(unit_findings);
            } else {
                pipeline.replay(unit);
            }
        }

        all_findings
    }

    fn lint_via_bridge(changed_ids: &[&str]) -> Vec<Finding> {
        let master_xml = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/repos/liquibase-xml/changelog/master.xml");
        let base_dir = master_xml.parent().unwrap();

        let loader = BridgeLoader::new(bridge_jar_path());
        let mut raw_units = loader.load(&master_xml).expect("Failed to load via bridge");
        resolve_source_paths(&mut raw_units, base_dir);

        lint_loaded_units(raw_units, changed_ids)
    }

    fn sort_findings(findings: &mut [Finding]) {
        findings.sort_by(|a, b| {
            a.rule_id
                .cmp(&b.rule_id)
                .then_with(|| a.file.cmp(&b.file))
                .then_with(|| a.start_line.cmp(&b.start_line))
        });
    }

    #[test]
    fn test_bridge_parses_all_changesets() {
        let master_xml = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/repos/liquibase-xml/changelog/master.xml");

        let loader = BridgeLoader::new(bridge_jar_path());
        let raw_units = loader.load(&master_xml).expect("Failed to load via bridge");

        assert!(
            !raw_units.is_empty(),
            "Bridge should produce at least one changeset"
        );

        // The fixture contains 39 changesets. The bridge may produce fewer if
        // some changesets generate no SQL. Assert a reasonable range.
        assert!(
            raw_units.len() >= 30 && raw_units.len() <= 45,
            "Expected 30-45 changesets from bridge, got {}",
            raw_units.len()
        );
    }

    #[test]
    fn test_bridge_lint_all_findings() {
        let mut findings = lint_via_bridge(&[]);
        sort_findings(&mut findings);

        insta::assert_yaml_snapshot!(findings, {
            "[].file" => insta::dynamic_redaction(|value, _path| {
                let s = value.as_str().unwrap();
                let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
                filename.to_string()
            })
        });
    }

    #[test]
    fn test_bridge_lint_004_only() {
        let mut findings = lint_via_bridge(&[
            "004-add-users-email-index",
            "004-add-subscriptions-account-index",
            "004-add-products-composite-index",
        ]);
        sort_findings(&mut findings);

        insta::assert_yaml_snapshot!(findings, {
            "[].file" => insta::dynamic_redaction(|value, _path| {
                let s = value.as_str().unwrap();
                let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
                filename.to_string()
            })
        });
    }

    #[test]
    fn test_bridge_lint_005_only() {
        let mut findings = lint_via_bridge(&[
            "005-add-fk-orders-user",
            "005-add-fk-subscriptions-account",
            "005-add-fk-orders-account",
        ]);
        sort_findings(&mut findings);

        insta::assert_yaml_snapshot!(findings, {
            "[].file" => insta::dynamic_redaction(|value, _path| {
                let s = value.as_str().unwrap();
                let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
                filename.to_string()
            })
        });
    }

    #[test]
    fn test_bridge_lint_006_only() {
        let mut findings =
            lint_via_bridge(&["006-create-event-log", "006-create-subscription-invoices"]);
        sort_findings(&mut findings);

        insta::assert_yaml_snapshot!(findings, {
            "[].file" => insta::dynamic_redaction(|value, _path| {
                let s = value.as_str().unwrap();
                let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
                filename.to_string()
            })
        });
    }

    #[test]
    fn test_bridge_lint_008_only() {
        let mut findings = lint_via_bridge(&[
            "008-add-region-to-accounts",
            "008-add-priority-to-orders",
            "008-add-category-to-products",
        ]);
        sort_findings(&mut findings);

        insta::assert_yaml_snapshot!(findings, {
            "[].file" => insta::dynamic_redaction(|value, _path| {
                let s = value.as_str().unwrap();
                let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
                filename.to_string()
            })
        });
    }

    #[test]
    fn test_bridge_lint_010_only() {
        let mut findings = lint_via_bridge(&[
            "010-truncate-event-log",
            "010-drop-unused-indexes",
            "010-drop-event-log",
            "010-drop-index-if-exists",
            "010-drop-table-if-exists",
        ]);
        sort_findings(&mut findings);

        // The plain DROP INDEX / DROP TABLE should trigger PGM401
        let pgm401: Vec<_> = findings
            .iter()
            .filter(|f| f.rule_id.as_str() == "PGM401")
            .collect();
        assert_eq!(
            pgm401.len(),
            2,
            "Expected exactly 2 PGM401 findings (DROP INDEX + DROP TABLE without IF EXISTS).\n\
         The IF EXISTS variants must NOT fire.\nAll findings: {:?}",
            findings
                .iter()
                .map(|f| format!("{}: {}", f.rule_id, f.message))
                .collect::<Vec<_>>()
        );

        // TRUNCATE TABLE event_log CASCADE should trigger PGM203 + PGM204
        let pgm203_bridge: Vec<_> = findings
            .iter()
            .filter(|f| f.rule_id.as_str() == "PGM203")
            .collect();
        assert_eq!(
            pgm203_bridge.len(),
            1,
            "Expected exactly 1 PGM203 finding (TRUNCATE TABLE on existing table).\nAll findings: {:?}",
            findings
                .iter()
                .map(|f| format!("{}: {}", f.rule_id, f.message))
                .collect::<Vec<_>>()
        );
        let pgm204_bridge: Vec<_> = findings
            .iter()
            .filter(|f| f.rule_id.as_str() == "PGM204")
            .collect();
        assert_eq!(
            pgm204_bridge.len(),
            1,
            "Expected exactly 1 PGM204 finding (TRUNCATE TABLE CASCADE on existing table).\nAll findings: {:?}",
            findings
                .iter()
                .map(|f| format!("{}: {}", f.rule_id, f.message))
                .collect::<Vec<_>>()
        );

        insta::assert_yaml_snapshot!(findings, {
            "[].file" => insta::dynamic_redaction(|value, _path| {
                let s = value.as_str().unwrap();
                let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
                filename.to_string()
            })
        });
    }

    fn liquibase_binary_path() -> PathBuf {
        PathBuf::from(
            std::env::var("PG_LINT_LIQUIBASE_PATH").unwrap_or_else(|_| "liquibase".to_string()),
        )
    }

    fn lint_via_updatesql(changed_ids: &[&str]) -> Vec<Finding> {
        let master_xml = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/repos/liquibase-xml/changelog/master.xml");
        let base_dir = master_xml.parent().unwrap();

        let loader = UpdateSqlLoader::new(liquibase_binary_path());
        let mut raw_units = loader
            .load(&master_xml)
            .expect("Failed to load via update-sql");
        resolve_source_paths(&mut raw_units, base_dir);

        lint_loaded_units(raw_units, changed_ids)
    }

    #[test]
    fn test_updatesql_parses_all_changesets() {
        let master_xml = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/repos/liquibase-xml/changelog/master.xml");

        let loader = UpdateSqlLoader::new(liquibase_binary_path());
        let raw_units = loader
            .load(&master_xml)
            .expect("Failed to load via update-sql");

        assert!(
            !raw_units.is_empty(),
            "Update-sql should produce at least one changeset"
        );

        // The fixture contains 39 changesets. update-sql may produce fewer if
        // some changesets generate no SQL. Assert a reasonable range.
        assert!(
            raw_units.len() >= 30 && raw_units.len() <= 45,
            "Expected 30-45 changesets from update-sql, got {}",
            raw_units.len()
        );
    }

    #[test]
    fn test_updatesql_lint_all_findings() {
        let mut findings = lint_via_updatesql(&[]);
        sort_findings(&mut findings);

        // NOTE: The update-sql path produces 2 extra PGM003 findings compared to
        // the bridge snapshot. This is because update-sql cannot detect
        // `runInTransaction="false"` from the XML -- it always assumes
        // `run_in_transaction: true`. As a result, CONCURRENTLY-in-transaction
        // warnings (PGM003) fire for changesets 009 and 010 that the bridge
        // correctly suppresses (since it knows those changesets run outside a
        // transaction).
        insta::assert_yaml_snapshot!(findings, {
            "[].file" => insta::dynamic_redaction(|value, _path| {
                let s = value.as_str().unwrap();
                let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
                filename.to_string()
            })
        });
    }

    #[test]
    fn test_updatesql_lint_004_only() {
        let mut findings = lint_via_updatesql(&[
            "004-add-users-email-index",
            "004-add-subscriptions-account-index",
            "004-add-products-composite-index",
        ]);
        sort_findings(&mut findings);

        insta::assert_yaml_snapshot!(findings, {
            "[].file" => insta::dynamic_redaction(|value, _path| {
                let s = value.as_str().unwrap();
                let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
                filename.to_string()
            })
        });
    }

    #[test]
    fn test_updatesql_lint_005_only() {
        let mut findings = lint_via_updatesql(&[
            "005-add-fk-orders-user",
            "005-add-fk-subscriptions-account",
            "005-add-fk-orders-account",
        ]);
        sort_findings(&mut findings);

        insta::assert_yaml_snapshot!(findings, {
            "[].file" => insta::dynamic_redaction(|value, _path| {
                let s = value.as_str().unwrap();
                let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
                filename.to_string()
            })
        });
    }

    #[test]
    fn test_updatesql_lint_006_only() {
        let mut findings =
            lint_via_updatesql(&["006-create-event-log", "006-create-subscription-invoices"]);
        sort_findings(&mut findings);

        insta::assert_yaml_snapshot!(findings, {
            "[].file" => insta::dynamic_redaction(|value, _path| {
                let s = value.as_str().unwrap();
                let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
                filename.to_string()
            })
        });
    }

    #[test]
    fn test_updatesql_lint_010_only() {
        let mut findings = lint_via_updatesql(&[
            "010-truncate-event-log",
            "010-drop-unused-indexes",
            "010-drop-event-log",
            "010-drop-index-if-exists",
            "010-drop-table-if-exists",
        ]);
        sort_findings(&mut findings);

        // The plain DROP INDEX / DROP TABLE should trigger PGM401
        let pgm401_usql: Vec<_> = findings
            .iter()
            .filter(|f| f.rule_id.as_str() == "PGM401")
            .collect();
        assert_eq!(
            pgm401_usql.len(),
            2,
            "Expected exactly 2 PGM401 findings (DROP INDEX + DROP TABLE without IF EXISTS).\n\
         The IF EXISTS variants must NOT fire.\nAll findings: {:?}",
            findings
                .iter()
                .map(|f| format!("{}: {}", f.rule_id, f.message))
                .collect::<Vec<_>>()
        );

        // TRUNCATE TABLE event_log CASCADE should trigger PGM203 + PGM204
        let pgm203_usql: Vec<_> = findings
            .iter()
            .filter(|f| f.rule_id.as_str() == "PGM203")
            .collect();
        assert_eq!(
            pgm203_usql.len(),
            1,
            "Expected exactly 1 PGM203 finding (TRUNCATE TABLE on existing table).\nAll findings: {:?}",
            findings
                .iter()
                .map(|f| format!("{}: {}", f.rule_id, f.message))
                .collect::<Vec<_>>()
        );
        let pgm204_usql: Vec<_> = findings
            .iter()
            .filter(|f| f.rule_id.as_str() == "PGM204")
            .collect();
        assert_eq!(
            pgm204_usql.len(),
            1,
            "Expected exactly 1 PGM204 finding (TRUNCATE TABLE CASCADE on existing table).\nAll findings: {:?}",
            findings
                .iter()
                .map(|f| format!("{}: {}", f.rule_id, f.message))
                .collect::<Vec<_>>()
        );

        insta::assert_yaml_snapshot!(findings, {
            "[].file" => insta::dynamic_redaction(|value, _path| {
                let s = value.as_str().unwrap();
                let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
                filename.to_string()
            })
        });
    }

    fn lint_bridge_multi_schema(changed_ids: &[&str]) -> Vec<Finding> {
        let master_xml = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/repos/liquibase-multi-schema/changelog/master.xml");
        let base_dir = master_xml.parent().unwrap();

        let loader = BridgeLoader::new(bridge_jar_path());
        let mut raw_units = loader
            .load(&master_xml)
            .expect("Failed to load multi-schema via bridge");
        resolve_source_paths(&mut raw_units, base_dir);

        lint_loaded_units(raw_units, changed_ids)
    }

    #[test]
    fn test_bridge_multi_schema_all_findings() {
        let mut findings = lint_bridge_multi_schema(&[]);
        sort_findings(&mut findings);
        let findings = common::normalize_findings(findings, "liquibase-multi-schema");
        insta::assert_yaml_snapshot!(findings);
    }

    fn lint_updatesql_multi_schema(changed_ids: &[&str]) -> Vec<Finding> {
        let master_xml = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/repos/liquibase-multi-schema/changelog/master.xml");
        let base_dir = master_xml.parent().unwrap();

        let loader = UpdateSqlLoader::new(liquibase_binary_path());
        let mut raw_units = loader
            .load(&master_xml)
            .expect("Failed to load multi-schema via update-sql");
        resolve_source_paths(&mut raw_units, base_dir);

        lint_loaded_units(raw_units, changed_ids)
    }

    #[test]
    fn test_updatesql_multi_schema_all_findings() {
        let mut findings = lint_updatesql_multi_schema(&[]);
        sort_findings(&mut findings);
        let findings = common::normalize_findings(findings, "liquibase-multi-schema");
        insta::assert_yaml_snapshot!(findings);
    }
}
