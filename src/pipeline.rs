//! Lint pipeline — extracts the shared replay-and-lint loop body.
//!
//! The [`LintPipeline`] struct encapsulates the single-pass replay strategy:
//! catalog state, table-creation tracking, and the clone → replay → lint → cap
//! sequence that was previously duplicated across `main.rs` and integration tests.

use std::collections::HashSet;

use crate::Catalog;
use crate::catalog::replay;
use crate::input::MigrationUnit;
use crate::names::RelationName;
use crate::parser::ir::IrNode;
use crate::rules::{self, Finding, LintContext, Rule, RuleId};

/// Encapsulates the single-pass replay + lint pipeline.
///
/// Callers feed migration units one at a time via [`replay`] (non-changed)
/// or [`lint`] (changed). The pipeline owns the catalog and the
/// `tables_created_in_change` set, handling the clone-replay-track-cap
/// sequence internally. Only up units change them: a down unit
/// (`MigrationUnit::is_down`) undoes an up rather than following it, so it is
/// linted against them and never replayed into them.
///
/// [`replay`]: LintPipeline::replay
/// [`lint`]: LintPipeline::lint
pub struct LintPipeline {
    catalog: Catalog,
    tables_created_in_change: HashSet<RelationName>,
}

impl LintPipeline {
    /// Create a new pipeline with an empty catalog.
    pub fn new() -> Self {
        Self {
            catalog: Catalog::new(),
            tables_created_in_change: HashSet::new(),
        }
    }

    /// Replay a unit without linting (for non-changed migration files).
    ///
    /// Applies an up unit's statements to the catalog so that subsequent
    /// units see the correct schema state. A down unit changes nothing.
    pub fn replay(&mut self, unit: &MigrationUnit) {
        if unit.is_down {
            return;
        }
        replay::apply(&mut self.catalog, unit);
    }

    /// Lint a changed unit. Returns raw findings (before suppression).
    ///
    /// An up unit is replayed as [`replay`](LintPipeline::replay) replays it,
    /// and the tables it creates are added to `tables_created_in_change`, so
    /// later units see both. A down unit changes neither; see `lint_down`.
    ///
    /// `rules` also sets `LintContext::pgm027_enabled`: with a set that lacks
    /// PGM027, the rules that leave a statement PostgreSQL rejects to PGM027
    /// report it as they would a statement PostgreSQL accepts.
    pub fn lint(&mut self, unit: &MigrationUnit, rules: &[RuleId]) -> Vec<Finding> {
        if unit.is_down {
            self.lint_down(unit, rules)
        } else {
            self.lint_up(unit, rules)
        }
    }

    /// Replay an up unit, record the tables it creates, then lint it against
    /// the catalog before and after it.
    fn lint_up(&mut self, unit: &MigrationUnit, rules: &[RuleId]) -> Vec<Finding> {
        let catalog_before = self.catalog.clone();
        replay::apply(&mut self.catalog, unit);
        self.tables_created_in_change
            .extend(tables_created_by(unit, &catalog_before).cloned());

        run_rules(
            unit,
            rules,
            &catalog_before,
            &self.catalog,
            &self.tables_created_in_change,
        )
    }

    /// Lint a down unit against the schema at its position in the history,
    /// without changing it: `catalog_before` is the pipeline's catalog, and
    /// `catalog_after` and `tables_created_in_change` hold the down's own
    /// effect in copies dropped once the rules have run. Its findings are
    /// capped to INFO (PGM901).
    fn lint_down(&self, unit: &MigrationUnit, rules: &[RuleId]) -> Vec<Finding> {
        let mut catalog_after = self.catalog.clone();
        replay::apply(&mut catalog_after, unit);
        let mut tables_created_in_change = self.tables_created_in_change.clone();
        tables_created_in_change.extend(tables_created_by(unit, &self.catalog).cloned());

        let mut findings = run_rules(
            unit,
            rules,
            &self.catalog,
            &catalog_after,
            &tables_created_in_change,
        );
        rules::cap_for_down_migration(&mut findings);
        findings
    }
}

/// The tables `unit` creates, run against `catalog_before`, for
/// `LintContext::is_existing_table` to treat as new. A `CREATE TABLE IF NOT
/// EXISTS` of a table `catalog_before` already holds is a no-op, not a
/// creation, and must not mask rules on later statements, so it is left out.
fn tables_created_by<'a>(
    unit: &'a MigrationUnit,
    catalog_before: &'a Catalog,
) -> impl Iterator<Item = &'a RelationName> {
    unit.statements
        .iter()
        .filter_map(move |stmt| match &stmt.node {
            IrNode::CreateTable(ct)
                if !(ct.if_not_exists && catalog_before.has_table(ct.name.relation())) =>
            {
                Some(ct.name.relation())
            }
            _ => None,
        })
}

/// Run `rules` on `unit` in a [`LintContext`] built from the given schema
/// state.
fn run_rules(
    unit: &MigrationUnit,
    rules: &[RuleId],
    catalog_before: &Catalog,
    catalog_after: &Catalog,
    tables_created_in_change: &HashSet<RelationName>,
) -> Vec<Finding> {
    let ctx = LintContext {
        catalog_before,
        catalog_after,
        tables_created_in_change,
        run_in_transaction: unit.run_in_transaction,
        is_down: unit.is_down,
        pgm027_enabled: rules.contains(&RuleId::Pgm027),
        file: &unit.source_file,
    };

    rules
        .iter()
        .flat_map(|rule| rule.check(&unit.statements, &ctx))
        .collect()
}

impl Default for LintPipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::catalog::builder::{CatalogAssertions, TEST_DEFAULT_SCHEMA, test_relation};
    use crate::normalize::resolve_units;
    use crate::parser::pg_query::parse_sql;
    use crate::rules::Severity;

    const CREATE_USERS: &str =
        "CREATE TABLE users (id bigint PRIMARY KEY, email text NOT NULL UNIQUE);";
    const INDEX_USERS_EMAIL: &str = "CREATE INDEX idx_users_email ON users (email);";
    const DROP_USERS_CREATE_AUDIT: &str =
        "DROP TABLE users; CREATE TABLE audit (id bigint PRIMARY KEY);";

    fn unit(file: &str, sql: &str, is_down: bool) -> MigrationUnit {
        let parsed = MigrationUnit {
            id: file.to_string(),
            statements: parse_sql(sql),
            source_file: PathBuf::from(file),
            source_line_offset: 1,
            run_in_transaction: true,
            is_down,
        };
        resolve_units(vec![parsed], TEST_DEFAULT_SCHEMA)
            .pop()
            .expect("one unit in, one unit out")
    }

    fn up(file: &str, sql: &str) -> MigrationUnit {
        unit(file, sql, false)
    }

    fn down(file: &str, sql: &str) -> MigrationUnit {
        unit(file, sql, true)
    }

    fn pipeline_with_users_replayed() -> LintPipeline {
        let mut pipeline = LintPipeline::new();
        pipeline.replay(&up("V001__create_users.sql", CREATE_USERS));
        pipeline
    }

    fn pipeline_with_users_linted() -> LintPipeline {
        let mut pipeline = LintPipeline::new();
        pipeline.lint(&up("V001__create_users.sql", CREATE_USERS), &[]);
        pipeline
    }

    fn rule_ids(findings: &[Finding]) -> Vec<RuleId> {
        let mut ids: Vec<RuleId> = findings.iter().map(|finding| finding.rule_id).collect();
        ids.sort();
        ids
    }

    #[test]
    fn replaying_a_down_leaves_the_catalog_and_created_tables_alone() {
        let mut pipeline = pipeline_with_users_linted();
        let catalog_before = pipeline.catalog.clone();
        let created_before = pipeline.tables_created_in_change.clone();

        pipeline.replay(&down(
            "V001__create_users_down.sql",
            DROP_USERS_CREATE_AUDIT,
        ));

        pipeline.catalog.assert_same_state_as(&catalog_before);
        assert_eq!(pipeline.tables_created_in_change, created_before);
    }

    #[test]
    fn linting_a_down_leaves_the_catalog_and_created_tables_alone() {
        let mut pipeline = pipeline_with_users_linted();
        let catalog_before = pipeline.catalog.clone();
        let created_before = pipeline.tables_created_in_change.clone();

        pipeline.lint(
            &down("V001__create_users_down.sql", DROP_USERS_CREATE_AUDIT),
            &[],
        );

        pipeline.catalog.assert_same_state_as(&catalog_before);
        assert_eq!(pipeline.tables_created_in_change, created_before);
    }

    #[test]
    fn a_linted_down_sees_the_schema_at_its_position_as_catalog_before() {
        let findings = pipeline_with_users_replayed().lint(
            &down("V001__create_users_down.sql", "DROP TABLE users;"),
            &[RuleId::Pgm201],
        );

        assert_eq!(rule_ids(&findings), [RuleId::Pgm201]);
    }

    #[test]
    fn a_linted_down_sees_its_own_effect_in_catalog_after() {
        let findings = pipeline_with_users_replayed().lint(
            &down("V002__drop_idx_users_email_down.sql", INDEX_USERS_EMAIL),
            &[RuleId::Pgm508],
        );

        assert_eq!(rule_ids(&findings), [RuleId::Pgm508]);
    }

    #[test]
    fn a_linted_down_has_its_findings_capped_to_info() {
        let findings = pipeline_with_users_replayed().lint(
            &down("V002__drop_idx_users_email_down.sql", INDEX_USERS_EMAIL),
            &[RuleId::Pgm001],
        );

        let [finding] = findings.as_slice() else {
            panic!("expected one PGM001 finding, got {findings:?}");
        };
        assert_eq!(finding.severity, Severity::Info);
    }

    #[test]
    fn a_linted_down_treats_tables_created_by_earlier_changed_units_as_new() {
        let findings = pipeline_with_users_linted().lint(
            &down("V002__drop_idx_users_email_down.sql", INDEX_USERS_EMAIL),
            &[RuleId::Pgm001],
        );

        assert_eq!(rule_ids(&findings), []);
    }

    #[test]
    fn a_linted_down_treats_tables_it_creates_itself_as_new() {
        let recreate_users_then_index =
            format!("DROP TABLE users; {CREATE_USERS} {INDEX_USERS_EMAIL}");

        let findings = pipeline_with_users_replayed().lint(
            &down("V002__drop_users_down.sql", &recreate_users_then_index),
            &[RuleId::Pgm001],
        );

        assert_eq!(rule_ids(&findings), []);
    }

    #[test]
    fn a_linted_down_does_not_count_create_table_if_not_exists_of_an_existing_table_as_new() {
        let findings = pipeline_with_users_replayed().lint(
            &down(
                "V002__drop_users_down.sql",
                &format!(
                    "CREATE TABLE IF NOT EXISTS users (id bigint PRIMARY KEY); {INDEX_USERS_EMAIL}"
                ),
            ),
            &[RuleId::Pgm001],
        );

        assert_eq!(rule_ids(&findings), [RuleId::Pgm001]);
    }

    #[test]
    fn linting_an_up_applies_it_and_records_the_tables_it_creates() {
        let pipeline = pipeline_with_users_linted();

        pipeline.catalog.assert_table("users");
        assert_eq!(
            pipeline.tables_created_in_change,
            HashSet::from([test_relation("users")])
        );
    }

    #[test]
    fn linting_an_up_keeps_its_effect_for_later_units() {
        let mut pipeline = pipeline_with_users_replayed();

        pipeline.lint(&up("V002__drop_users.sql", "DROP TABLE users;"), &[]);

        pipeline.catalog.assert_no_table("users");
    }

    #[test]
    fn linting_an_up_on_a_replayed_table_reports_at_full_severity() {
        let findings = pipeline_with_users_replayed().lint(
            &up("V002__index_users_email.sql", INDEX_USERS_EMAIL),
            &[RuleId::Pgm001],
        );

        let [finding] = findings.as_slice() else {
            panic!("expected one PGM001 finding, got {findings:?}");
        };
        assert_eq!(finding.severity, Severity::Critical);
    }

    #[test]
    fn linting_an_up_treats_tables_created_by_earlier_changed_units_as_new() {
        let findings = pipeline_with_users_linted().lint(
            &up("V002__index_users_email.sql", INDEX_USERS_EMAIL),
            &[RuleId::Pgm001],
        );

        assert_eq!(rule_ids(&findings), []);
    }

    #[test]
    fn linting_an_up_does_not_count_create_table_if_not_exists_of_an_existing_table_as_new() {
        let mut pipeline = pipeline_with_users_replayed();

        pipeline.lint(
            &up(
                "V002__create_users_again.sql",
                "CREATE TABLE IF NOT EXISTS users (id bigint PRIMARY KEY);",
            ),
            &[],
        );

        assert_eq!(pipeline.tables_created_in_change, HashSet::new());
    }
}
