use std::collections::HashSet;

use pg_migration_lint::{
    RuleId, RuleInfo,
    output::{Reporter, SarifReporter, SonarQubeReporter},
};

use crate::common::{changed_files_for, lint_fixture};

mod common;

#[test]
fn test_sarif_output_valid_structure() {
    // Run the all-rules fixture through the full pipeline, emit SARIF, and
    // verify the output is valid SARIF 2.1.0 with correct structure.
    let changed = changed_files_for("all-rules");
    let findings = lint_fixture("all-rules", &changed);
    assert!(
        !findings.is_empty(),
        "All-rules fixture should produce findings"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let reporter = SarifReporter::new();
    reporter.emit(&findings, dir.path()).expect("emit SARIF");

    let content =
        std::fs::read_to_string(dir.path().join("findings.sarif")).expect("read SARIF file");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse SARIF JSON");

    // Verify it's valid SARIF 2.1.0
    assert_eq!(
        parsed["$schema"],
        "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json",
        "SARIF $schema field must be the 2.1.0 schema URL"
    );
    assert_eq!(parsed["version"], "2.1.0", "SARIF version must be 2.1.0");

    // Verify runs array
    let runs = parsed["runs"].as_array().expect("runs should be an array");
    assert_eq!(runs.len(), 1, "Should have exactly 1 run");

    // Verify tool driver
    let driver = &runs[0]["tool"]["driver"];
    assert_eq!(driver["name"], "pg-migration-lint");
    assert!(driver["version"].is_string(), "driver should have version");
    assert!(
        driver["informationUri"].is_string(),
        "driver should have informationUri"
    );

    // Verify results count matches findings
    let results = runs[0]["results"]
        .as_array()
        .expect("results should be an array");
    assert_eq!(
        results.len(),
        findings.len(),
        "SARIF results count should match findings count"
    );

    // Verify all results have correct ruleIds from our rule set
    let known_rules: HashSet<&str> = RuleId::lint_rules().map(|r| r.as_str()).collect();
    for result in results {
        let rule_id = result["ruleId"]
            .as_str()
            .expect("ruleId should be a string");
        assert!(
            known_rules.contains(rule_id),
            "SARIF result ruleId '{}' should be a known rule",
            rule_id
        );
    }

    // Verify file paths in results are not empty and reference SQL files
    for result in results {
        let uri = result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
            .as_str()
            .expect("artifactLocation.uri should be a string");
        assert!(
            !uri.is_empty(),
            "SARIF artifactLocation.uri should not be empty"
        );
        assert!(
            uri.contains(".sql"),
            "SARIF file paths should reference SQL files, got: {}",
            uri
        );
    }

    // Verify rules array has entries for distinct rule IDs in findings
    let rules = driver["rules"]
        .as_array()
        .expect("rules should be an array");
    let finding_rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();
    assert_eq!(
        rules.len(),
        finding_rule_ids.len(),
        "SARIF rules array should have one entry per distinct rule ID"
    );
    for rule in rules {
        assert!(rule["id"].is_string(), "Each rule must have an id");
        assert!(
            rule["shortDescription"]["text"].is_string(),
            "Each rule must have shortDescription.text"
        );
        assert!(
            rule["defaultConfiguration"]["level"].is_string(),
            "Each rule must have defaultConfiguration.level"
        );
        let level = rule["defaultConfiguration"]["level"].as_str().unwrap();
        assert!(
            ["error", "warning", "note"].contains(&level),
            "Rule level must be error, warning, or note; got: {}",
            level
        );
    }

    // Verify line numbers are positive and endLine >= startLine
    for result in results {
        let region = &result["locations"][0]["physicalLocation"]["region"];
        let start_line = region["startLine"]
            .as_u64()
            .expect("startLine should be a number");
        let end_line = region["endLine"]
            .as_u64()
            .expect("endLine should be a number");
        assert!(start_line >= 1, "startLine should be >= 1");
        assert!(end_line >= start_line, "endLine should be >= startLine");
    }

    // Verify SARIF levels map correctly to known values
    for result in results {
        let level = result["level"].as_str().expect("level should be a string");
        assert!(
            ["error", "warning", "note"].contains(&level),
            "Result level must be error, warning, or note; got: {}",
            level
        );
    }
}

#[test]
fn test_sarif_output_round_trip_from_fixture() {
    // A focused round-trip test: emit SARIF from a small changed-file set,
    // parse it back, and verify specific finding data survives serialization.
    let findings = lint_fixture("all-rules", &["V002__violations.sql"]);
    assert!(!findings.is_empty(), "V002 should produce findings");

    let dir = tempfile::tempdir().expect("tempdir");
    let reporter = SarifReporter::new();
    reporter.emit(&findings, dir.path()).expect("emit SARIF");

    let content =
        std::fs::read_to_string(dir.path().join("findings.sarif")).expect("read SARIF file");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse SARIF JSON");

    let results = parsed["runs"][0]["results"]
        .as_array()
        .expect("results array");

    // For each original finding, verify it appears in the SARIF output
    for finding in &findings {
        let matching = results.iter().find(|r| {
            r["ruleId"].as_str() == Some(finding.rule_id.as_str())
                && r["message"]["text"].as_str() == Some(&finding.message)
        });
        assert!(
            matching.is_some(),
            "Finding {} with message '{}' should appear in SARIF output",
            finding.rule_id,
            finding.message
        );

        let matched = matching.unwrap();
        let loc = &matched["locations"][0]["physicalLocation"];
        assert_eq!(
            loc["region"]["startLine"].as_u64().unwrap() as usize,
            finding.start_line,
            "startLine mismatch for {}",
            finding.rule_id
        );
        assert_eq!(
            loc["region"]["endLine"].as_u64().unwrap() as usize,
            finding.end_line,
            "endLine mismatch for {}",
            finding.rule_id
        );
    }
}

#[test]
fn test_sonarqube_output_valid_structure() {
    // Run the all-rules fixture through the full pipeline, emit SonarQube JSON,
    // and verify the output has the correct Generic Issue Import structure.
    let changed = changed_files_for("all-rules");
    let findings = lint_fixture("all-rules", &changed);
    assert!(
        !findings.is_empty(),
        "All-rules fixture should produce findings"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let reporter = SonarQubeReporter::new(RuleInfo::all());
    reporter
        .emit(&findings, dir.path())
        .expect("emit SonarQube JSON");

    let content =
        std::fs::read_to_string(dir.path().join("findings.json")).expect("read SonarQube file");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse SonarQube JSON");

    // Verify top-level structure: both rules and issues arrays
    let rules = parsed["rules"]
        .as_array()
        .expect("rules should be an array");
    let issues = parsed["issues"]
        .as_array()
        .expect("issues should be an array");
    assert_eq!(
        issues.len(),
        findings.len(),
        "SonarQube issues count should match findings count"
    );

    // Verify rules array has required fields
    for rule in rules {
        assert_eq!(
            rule["engineId"], "pg-migration-lint",
            "All rules must have engineId 'pg-migration-lint'"
        );
        assert!(rule["id"].is_string(), "rule must have id");
        assert!(rule["name"].is_string(), "rule must have name");
        assert!(
            rule["cleanCodeAttribute"].is_string(),
            "rule must have cleanCodeAttribute"
        );
        assert!(rule["type"].is_string(), "rule must have type");
        assert!(rule["severity"].is_string(), "rule must have severity");
        let impacts = rule["impacts"].as_array().expect("impacts array");
        assert!(!impacts.is_empty(), "rule must have at least one impact");
    }

    // Verify each issue has the required fields
    let known_rules: HashSet<&str> = RuleId::lint_rules().map(|r| r.as_str()).collect();

    for issue in issues {
        // ruleId
        let rule_id = issue["ruleId"].as_str().expect("ruleId should be a string");
        assert!(
            known_rules.contains(rule_id),
            "SonarQube ruleId '{}' should be a known rule",
            rule_id
        );

        // effortMinutes
        assert!(
            issue["effortMinutes"].is_u64(),
            "issue must have effortMinutes"
        );

        // primaryLocation
        let primary_location = &issue["primaryLocation"];
        assert!(
            primary_location["message"].is_string(),
            "primaryLocation must have a message"
        );
        let message = primary_location["message"]
            .as_str()
            .expect("message string");
        assert!(
            !message.is_empty(),
            "primaryLocation.message should not be empty"
        );

        let file_path = primary_location["filePath"]
            .as_str()
            .expect("filePath should be a string");
        assert!(!file_path.is_empty(), "filePath should not be empty");
        assert!(
            file_path.contains(".sql"),
            "SonarQube file paths should reference SQL files, got: {}",
            file_path
        );

        // textRange
        let text_range = &primary_location["textRange"];
        let start_line = text_range["startLine"]
            .as_u64()
            .expect("startLine should be a number");
        let end_line = text_range["endLine"]
            .as_u64()
            .expect("endLine should be a number");
        assert!(start_line >= 1, "startLine should be >= 1");
        assert!(end_line >= start_line, "endLine should be >= startLine");
    }
}

#[test]
fn test_sonarqube_output_round_trip_from_fixture() {
    // Focused round-trip: emit SonarQube JSON from a small changed-file set,
    // parse it back, and verify each finding's data survives serialization.
    let findings = lint_fixture("all-rules", &["V002__violations.sql"]);
    assert!(!findings.is_empty(), "V002 should produce findings");

    let dir = tempfile::tempdir().expect("tempdir");
    let reporter = SonarQubeReporter::new(RuleInfo::all());
    reporter
        .emit(&findings, dir.path())
        .expect("emit SonarQube JSON");

    let content =
        std::fs::read_to_string(dir.path().join("findings.json")).expect("read SonarQube file");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse SonarQube JSON");

    let issues = parsed["issues"].as_array().expect("issues array");

    // For each original finding, verify it appears in the SonarQube output
    for finding in &findings {
        let matching = issues.iter().find(|issue| {
            issue["ruleId"].as_str() == Some(finding.rule_id.as_str())
                && issue["primaryLocation"]["message"].as_str() == Some(&finding.message)
        });
        assert!(
            matching.is_some(),
            "Finding {} with message '{}' should appear in SonarQube output",
            finding.rule_id,
            finding.message
        );

        let matched = matching.unwrap();

        // Verify line numbers
        let text_range = &matched["primaryLocation"]["textRange"];
        assert_eq!(
            text_range["startLine"].as_u64().unwrap() as usize,
            finding.start_line,
            "startLine mismatch for {}",
            finding.rule_id
        );
        assert_eq!(
            text_range["endLine"].as_u64().unwrap() as usize,
            finding.end_line,
            "endLine mismatch for {}",
            finding.rule_id
        );

        // Verify file path is not empty
        let file_path = matched["primaryLocation"]["filePath"]
            .as_str()
            .expect("filePath string");
        assert!(
            !file_path.is_empty(),
            "filePath should not be empty for {}",
            finding.rule_id
        );
    }
}

#[test]
fn test_sarif_and_sonarqube_finding_counts_match() {
    // Both reporters should produce the same number of entries from the same findings.
    let changed = changed_files_for("all-rules");
    let findings = lint_fixture("all-rules", &changed);

    let dir_sarif = tempfile::tempdir().expect("sarif tempdir");
    let dir_sonar = tempfile::tempdir().expect("sonar tempdir");

    let sarif_reporter = SarifReporter::new();
    sarif_reporter
        .emit(&findings, dir_sarif.path())
        .expect("emit SARIF");

    let sonar_reporter = SonarQubeReporter::new(RuleInfo::all());
    sonar_reporter
        .emit(&findings, dir_sonar.path())
        .expect("emit SonarQube");

    let sarif_content =
        std::fs::read_to_string(dir_sarif.path().join("findings.sarif")).expect("read SARIF");
    let sonar_content =
        std::fs::read_to_string(dir_sonar.path().join("findings.json")).expect("read SonarQube");

    let sarif_parsed: serde_json::Value =
        serde_json::from_str(&sarif_content).expect("parse SARIF");
    let sonar_parsed: serde_json::Value =
        serde_json::from_str(&sonar_content).expect("parse SonarQube");

    let sarif_count = sarif_parsed["runs"][0]["results"]
        .as_array()
        .expect("SARIF results")
        .len();
    let sonar_count = sonar_parsed["issues"]
        .as_array()
        .expect("SonarQube issues")
        .len();

    assert_eq!(
        sarif_count, sonar_count,
        "SARIF result count ({}) should match SonarQube issue count ({})",
        sarif_count, sonar_count
    );
    assert_eq!(
        sarif_count,
        findings.len(),
        "Both should match the original findings count ({})",
        findings.len()
    );
}
