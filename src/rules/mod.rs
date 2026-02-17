//! Rule engine and lint context
//!
//! Each rule implements the `Rule` trait and checks for specific migration safety issues.
//! Rules receive IR nodes and catalog state, returning findings with severity levels.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Serialize;
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

use crate::catalog::Catalog;
use crate::parser::ir::{IrNode, Located, SourceSpan};

#[cfg(test)]
pub mod test_helpers;

pub mod alter_table_check;
pub mod column_type_check;

mod pgm001;
mod pgm002;
mod pgm003;
mod pgm004;
mod pgm005;
mod pgm006;
mod pgm007;
mod pgm009;
mod pgm010;
mod pgm011;
mod pgm012;
mod pgm013;
mod pgm014;
mod pgm015;
mod pgm016;
mod pgm017;
mod pgm018;
mod pgm019;
mod pgm020;
mod pgm021;
mod pgm022;
mod pgm101;
mod pgm102;
mod pgm103;
mod pgm104;
mod pgm105;
mod pgm108;

// ---------------------------------------------------------------------------
// Rule ID enums
// ---------------------------------------------------------------------------

/// Strongly-typed rule identifier.
///
/// Wraps the three rule families so that match statements are exhaustive:
/// adding a new variant forces updates in `sonarqube_meta()`, `effort_minutes()`,
/// and everywhere else a rule ID is dispatched on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RuleId {
    /// Migration safety rules (0xx series).
    Migration(MigrationRule),
    /// Type-choice rules (1xx series).
    TypeChoice(TypeChoiceRule),
    /// Meta-behavior rules (9xx series).
    Meta(MetaRule),
}

impl RuleId {
    /// Zero-allocation string representation.
    pub fn as_str(&self) -> &'static str {
        use MigrationRule::*;
        use TypeChoiceRule::*;
        match self {
            RuleId::Migration(m) => match m {
                Pgm001 => "PGM001",
                Pgm002 => "PGM002",
                Pgm003 => "PGM003",
                Pgm004 => "PGM004",
                Pgm005 => "PGM005",
                Pgm006 => "PGM006",
                Pgm007 => "PGM007",
                Pgm009 => "PGM009",
                Pgm010 => "PGM010",
                Pgm011 => "PGM011",
                Pgm012 => "PGM012",
                Pgm013 => "PGM013",
                Pgm014 => "PGM014",
                Pgm015 => "PGM015",
                Pgm016 => "PGM016",
                Pgm017 => "PGM017",
                Pgm018 => "PGM018",
                Pgm019 => "PGM019",
                Pgm020 => "PGM020",
                Pgm021 => "PGM021",
                Pgm022 => "PGM022",
            },
            RuleId::TypeChoice(t) => match t {
                Pgm101 => "PGM101",
                Pgm102 => "PGM102",
                Pgm103 => "PGM103",
                Pgm104 => "PGM104",
                Pgm105 => "PGM105",
                Pgm108 => "PGM108",
            },
            RuleId::Meta(MetaRule::Pgm901) => "PGM901",
        }
    }
}

impl fmt::Display for RuleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for RuleId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for RuleId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s: String = serde::Deserialize::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl FromStr for RuleId {
    type Err = ParseRuleIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use MigrationRule::*;
        use TypeChoiceRule::*;
        match s {
            "PGM001" => Ok(RuleId::Migration(Pgm001)),
            "PGM002" => Ok(RuleId::Migration(Pgm002)),
            "PGM003" => Ok(RuleId::Migration(Pgm003)),
            "PGM004" => Ok(RuleId::Migration(Pgm004)),
            "PGM005" => Ok(RuleId::Migration(Pgm005)),
            "PGM006" => Ok(RuleId::Migration(Pgm006)),
            "PGM007" => Ok(RuleId::Migration(Pgm007)),
            "PGM009" => Ok(RuleId::Migration(Pgm009)),
            "PGM010" => Ok(RuleId::Migration(Pgm010)),
            "PGM011" => Ok(RuleId::Migration(Pgm011)),
            "PGM012" => Ok(RuleId::Migration(Pgm012)),
            "PGM013" => Ok(RuleId::Migration(Pgm013)),
            "PGM014" => Ok(RuleId::Migration(Pgm014)),
            "PGM015" => Ok(RuleId::Migration(Pgm015)),
            "PGM016" => Ok(RuleId::Migration(Pgm016)),
            "PGM017" => Ok(RuleId::Migration(Pgm017)),
            "PGM018" => Ok(RuleId::Migration(Pgm018)),
            "PGM019" => Ok(RuleId::Migration(Pgm019)),
            "PGM020" => Ok(RuleId::Migration(Pgm020)),
            "PGM021" => Ok(RuleId::Migration(Pgm021)),
            "PGM022" => Ok(RuleId::Migration(Pgm022)),
            "PGM101" => Ok(RuleId::TypeChoice(Pgm101)),
            "PGM102" => Ok(RuleId::TypeChoice(Pgm102)),
            "PGM103" => Ok(RuleId::TypeChoice(Pgm103)),
            "PGM104" => Ok(RuleId::TypeChoice(Pgm104)),
            "PGM105" => Ok(RuleId::TypeChoice(Pgm105)),
            "PGM108" => Ok(RuleId::TypeChoice(Pgm108)),
            "PGM901" => Ok(RuleId::Meta(MetaRule::Pgm901)),
            _ => Err(ParseRuleIdError(s.to_string())),
        }
    }
}

/// Error returned when a string cannot be parsed into a [`RuleId`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseRuleIdError(pub String);

impl From<RuleId> for Box<dyn Rule> {
    fn from(value: RuleId) -> Self {
        Box::new(value)
    }
}

impl Rule for RuleId {
    fn id(&self) -> Self {
        *self
    }

    fn default_severity(&self) -> Severity {
        match *self {
            Self::Migration(rule) => rule.into(),
            Self::TypeChoice(rule) => rule.into(),
            Self::Meta(rule) => rule.into(),
        }
    }

    fn description(&self) -> &'static str {
        match *self {
            Self::Migration(rule) => rule.description(),
            Self::TypeChoice(rule) => rule.description(),
            Self::Meta(rule) => rule.description(),
        }
    }

    fn explain(&self) -> &'static str {
        match *self {
            Self::Migration(rule) => rule.explain(),
            Self::TypeChoice(rule) => rule.explain(),
            Self::Meta(rule) => rule.explain(),
        }
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        match *self {
            Self::Migration(rule) => rule.check(*self, statements, ctx),
            Self::TypeChoice(rule) => rule.check(*self, statements, ctx),
            Self::Meta(rule) => rule.check(*self, statements, ctx),
        }
    }
}

/// Migration safety rules (PGM001–PGM022).
///
/// These detect locking, rewrite, and schema-integrity issues in DDL migrations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, EnumIter)]
pub enum MigrationRule {
    /// `CREATE INDEX` without `CONCURRENTLY` on existing tables.
    Pgm001,
    /// `DROP INDEX` without `CONCURRENTLY` on existing tables.
    Pgm002,
    /// Foreign keys without a covering index on the referencing table.
    Pgm003,
    /// Tables created without a primary key.
    Pgm004,
    /// Tables using `UNIQUE NOT NULL` instead of a proper `PRIMARY KEY`.
    Pgm005,
    /// Concurrent index operations inside a transaction.
    Pgm006,
    /// Volatile function defaults on columns.
    Pgm007,
    /// Column type changes on existing tables.
    Pgm009,
    /// Adding a `NOT NULL` column without a `DEFAULT` to an existing table.
    Pgm010,
    /// Dropping a column from an existing table.
    Pgm011,
    /// Adding a `PRIMARY KEY` without a prior unique index.
    Pgm012,
    /// Dropping a column that participates in a unique constraint or unique index.
    Pgm013,
    /// Dropping a column that participates in the table's primary key.
    Pgm014,
    /// Dropping a column that participates in a foreign key constraint.
    Pgm015,
    /// `SET NOT NULL` on an existing table column.
    Pgm016,
    /// Adding a `FOREIGN KEY` without `NOT VALID` on an existing table.
    Pgm017,
    /// Adding a `CHECK` constraint without `NOT VALID` on an existing table.
    Pgm018,
    /// `ALTER TABLE ... RENAME TO` on existing tables.
    Pgm019,
    /// `RENAME COLUMN` on an existing table.
    Pgm020,
    /// Adding a `UNIQUE` constraint without a pre-existing unique index.
    Pgm021,
    /// Dropping an existing table.
    Pgm022,
}

impl MigrationRule {
    fn description(&self) -> &'static str {
        match *self {
            Self::Pgm001 => pgm001::DESCRIPTION,
            Self::Pgm002 => pgm002::DESCRIPTION,
            Self::Pgm003 => pgm003::DESCRIPTION,
            Self::Pgm004 => pgm004::DESCRIPTION,
            Self::Pgm005 => pgm005::DESCRIPTION,
            Self::Pgm006 => pgm006::DESCRIPTION,
            Self::Pgm007 => pgm007::DESCRIPTION,
            Self::Pgm009 => pgm009::DESCRIPTION,
            Self::Pgm010 => pgm010::DESCRIPTION,
            Self::Pgm011 => pgm011::DESCRIPTION,
            Self::Pgm012 => pgm012::DESCRIPTION,
            Self::Pgm013 => pgm013::DESCRIPTION,
            Self::Pgm014 => pgm014::DESCRIPTION,
            Self::Pgm015 => pgm015::DESCRIPTION,
            Self::Pgm016 => pgm016::DESCRIPTION,
            Self::Pgm017 => pgm017::DESCRIPTION,
            Self::Pgm018 => pgm018::DESCRIPTION,
            Self::Pgm019 => pgm019::DESCRIPTION,
            Self::Pgm020 => pgm020::DESCRIPTION,
            Self::Pgm021 => pgm021::DESCRIPTION,
            Self::Pgm022 => pgm022::DESCRIPTION,
        }
    }

    fn explain(&self) -> &'static str {
        match *self {
            Self::Pgm001 => pgm001::EXPLAIN,
            Self::Pgm002 => pgm002::EXPLAIN,
            Self::Pgm003 => pgm003::EXPLAIN,
            Self::Pgm004 => pgm004::EXPLAIN,
            Self::Pgm005 => pgm005::EXPLAIN,
            Self::Pgm006 => pgm006::EXPLAIN,
            Self::Pgm007 => pgm007::EXPLAIN,
            Self::Pgm009 => pgm009::EXPLAIN,
            Self::Pgm010 => pgm010::EXPLAIN,
            Self::Pgm011 => pgm011::EXPLAIN,
            Self::Pgm012 => pgm012::EXPLAIN,
            Self::Pgm013 => pgm013::EXPLAIN,
            Self::Pgm014 => pgm014::EXPLAIN,
            Self::Pgm015 => pgm015::EXPLAIN,
            Self::Pgm016 => pgm016::EXPLAIN,
            Self::Pgm017 => pgm017::EXPLAIN,
            Self::Pgm018 => pgm018::EXPLAIN,
            Self::Pgm019 => pgm019::EXPLAIN,
            Self::Pgm020 => pgm020::EXPLAIN,
            Self::Pgm021 => pgm021::EXPLAIN,
            Self::Pgm022 => pgm022::EXPLAIN,
        }
    }

    fn check(
        &self,
        rule: impl Rule,
        statements: &[Located<IrNode>],
        ctx: &LintContext<'_>,
    ) -> Vec<Finding> {
        match *self {
            Self::Pgm001 => pgm001::check(rule, statements, ctx),
            Self::Pgm002 => pgm002::check(rule, statements, ctx),
            Self::Pgm003 => pgm003::check(rule, statements, ctx),
            Self::Pgm004 => pgm004::check(rule, statements, ctx),
            Self::Pgm005 => pgm005::check(rule, statements, ctx),
            Self::Pgm006 => pgm006::check(rule, statements, ctx),
            Self::Pgm007 => pgm007::check(rule, statements, ctx),
            Self::Pgm009 => pgm009::check(rule, statements, ctx),
            Self::Pgm010 => pgm010::check(rule, statements, ctx),
            Self::Pgm011 => pgm011::check(rule, statements, ctx),
            Self::Pgm012 => pgm012::check(rule, statements, ctx),
            Self::Pgm013 => pgm013::check(rule, statements, ctx),
            Self::Pgm014 => pgm014::check(rule, statements, ctx),
            Self::Pgm015 => pgm015::check(rule, statements, ctx),
            Self::Pgm016 => pgm016::check(rule, statements, ctx),
            Self::Pgm017 => pgm017::check(rule, statements, ctx),
            Self::Pgm018 => pgm018::check(rule, statements, ctx),
            Self::Pgm019 => pgm019::check(rule, statements, ctx),
            Self::Pgm020 => pgm020::check(rule, statements, ctx),
            Self::Pgm021 => pgm021::check(rule, statements, ctx),
            Self::Pgm022 => pgm022::check(rule, statements, ctx),
        }
    }
}

impl From<MigrationRule> for Severity {
    fn from(value: MigrationRule) -> Self {
        match value {
            MigrationRule::Pgm001
            | MigrationRule::Pgm002
            | MigrationRule::Pgm006
            | MigrationRule::Pgm009
            | MigrationRule::Pgm010
            | MigrationRule::Pgm016
            | MigrationRule::Pgm017
            | MigrationRule::Pgm018
            | MigrationRule::Pgm021 => Self::Critical,
            MigrationRule::Pgm003
            | MigrationRule::Pgm004
            | MigrationRule::Pgm012
            | MigrationRule::Pgm014 => Self::Major,
            MigrationRule::Pgm007
            | MigrationRule::Pgm013
            | MigrationRule::Pgm015
            | MigrationRule::Pgm022 => Self::Minor,
            MigrationRule::Pgm005
            | MigrationRule::Pgm011
            | MigrationRule::Pgm019
            | MigrationRule::Pgm020 => Self::Info,
        }
    }
}

/// "Don't Do This" type-choice rules (PGM101–PGM108).
///
/// These flag column types that should be avoided (e.g. `money`, `serial`, `char(n)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, EnumIter)]
pub enum TypeChoiceRule {
    /// `timestamp` without time zone.
    Pgm101,
    /// `timestamp(0)` or `timestamptz(0)`.
    Pgm102,
    /// `char(n)`.
    Pgm103,
    /// `money` type.
    Pgm104,
    /// `serial` / `bigserial` / `smallserial` column types.
    Pgm105,
    /// `json` type (use `jsonb` instead).
    Pgm108,
}

impl TypeChoiceRule {
    fn description(&self) -> &'static str {
        match *self {
            TypeChoiceRule::Pgm101 => pgm101::DESCRIPTION,
            TypeChoiceRule::Pgm102 => pgm102::DESCRIPTION,
            TypeChoiceRule::Pgm103 => pgm103::DESCRIPTION,
            TypeChoiceRule::Pgm104 => pgm104::DESCRIPTION,
            TypeChoiceRule::Pgm105 => pgm105::DESCRIPTION,
            TypeChoiceRule::Pgm108 => pgm108::DESCRIPTION,
        }
    }

    fn explain(&self) -> &'static str {
        match *self {
            Self::Pgm101 => pgm101::EXPLAIN,
            Self::Pgm102 => pgm102::EXPLAIN,
            Self::Pgm103 => pgm103::EXPLAIN,
            Self::Pgm104 => pgm104::EXPLAIN,
            Self::Pgm105 => pgm105::EXPLAIN,
            Self::Pgm108 => pgm108::EXPLAIN,
        }
    }

    fn check(
        &self,
        rule: impl Rule,
        statements: &[Located<IrNode>],
        ctx: &LintContext<'_>,
    ) -> Vec<Finding> {
        match *self {
            Self::Pgm101 => pgm101::check(rule, statements, ctx),
            Self::Pgm102 => pgm102::check(rule, statements, ctx),
            Self::Pgm103 => pgm103::check(rule, statements, ctx),
            Self::Pgm104 => pgm104::check(rule, statements, ctx),
            Self::Pgm105 => pgm105::check(rule, statements, ctx),
            Self::Pgm108 => pgm108::check(rule, statements, ctx),
        }
    }
}

impl From<TypeChoiceRule> for Severity {
    fn from(value: TypeChoiceRule) -> Self {
        match value {
            TypeChoiceRule::Pgm101
            | TypeChoiceRule::Pgm102
            | TypeChoiceRule::Pgm103
            | TypeChoiceRule::Pgm104
            | TypeChoiceRule::Pgm108 => Self::Minor,
            TypeChoiceRule::Pgm105 => Self::Info,
        }
    }
}

/// Meta-behavior rules (PGM9xx).
///
/// Not standalone lint rules — these label cross-cutting behaviors such as
/// down-migration severity capping (PGM901).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, EnumIter)]
pub enum MetaRule {
    Pgm901,
}

impl MetaRule {
    fn description(&self) -> &'static str {
        "Meta rules alter the behavior of other rules, they are not rules themselves"
    }

    fn explain(&self) -> &'static str {
        match *self {
            MetaRule::Pgm901 => {
                "This rule caps severity of triggered rules to INFO (not in SonarQube)"
            }
        }
    }

    fn check(&self, _: impl Rule, _: &[Located<IrNode>], _: &LintContext<'_>) -> Vec<Finding> {
        vec![]
    }
}

impl From<MetaRule> for Severity {
    fn from(_: MetaRule) -> Self {
        Self::Info
    }
}

impl fmt::Display for ParseRuleIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown rule ID: '{}'", self.0)
    }
}

impl std::error::Error for ParseRuleIdError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Severity {
    Info,
    Minor,
    Major,
    Critical,
    Blocker,
}

impl Severity {
    /// Parse from config string. Case-insensitive.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "info" => Some(Self::Info),
            "minor" => Some(Self::Minor),
            "major" => Some(Self::Major),
            "critical" => Some(Self::Critical),
            "blocker" => Some(Self::Blocker),
            _ => None,
        }
    }

    /// SonarQube severity string.
    pub fn sonarqube_str(&self) -> &'static str {
        match self {
            Severity::Info => "INFO",
            Severity::Minor => "MINOR",
            Severity::Major => "MAJOR",
            Severity::Critical => "CRITICAL",
            Severity::Blocker => "BLOCKER",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.sonarqube_str())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub rule_id: RuleId,
    pub severity: Severity,
    pub message: String,
    #[serde(serialize_with = "serialize_path_forward_slash")]
    pub file: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
}

#[allow(clippy::ptr_arg)] // serde serialize_with requires &PathBuf, not &Path
fn serialize_path_forward_slash<S: serde::Serializer>(
    path: &std::path::PathBuf,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.serialize_str(&path.to_string_lossy().replace('\\', "/"))
}

impl Finding {
    /// Create a finding from a rule, lint context, source span, and message.
    pub fn new(
        rule_id: RuleId,
        severity: Severity,
        message: String,
        file: &Path,
        span: &SourceSpan,
    ) -> Self {
        Self {
            rule_id,
            severity,
            message,
            file: file.to_path_buf(),
            start_line: span.start_line,
            end_line: span.end_line,
        }
    }
}

/// Controls which tables a rule considers "existing".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableScope {
    /// Table must exist in catalog_before AND not appear in tables_created_in_change.
    /// For locking/performance rules where brand-new tables are exempt.
    ExcludeCreatedInChange,
    /// Table must exist in catalog_before only.
    /// For side-effect/integrity rules where the warning matters even if the
    /// table was created earlier in the same set of changed files.
    AnyPreExisting,
}

/// Context available to rules during linting.
pub struct LintContext<'a> {
    /// The catalog state BEFORE the current unit was applied.
    /// Clone taken just before apply(). Used by PGM001/002 to check
    /// if a table is pre-existing.
    pub catalog_before: &'a Catalog,

    /// The catalog state AFTER the current unit was applied.
    /// Used for post-file checks (PGM003, PGM004, PGM005).
    pub catalog_after: &'a Catalog,

    /// Set of table names created in the current set of changed files.
    /// Built incrementally during the single-pass replay: when a changed
    /// file contains a CreateTable, add it to this set before linting
    /// subsequent changed files.
    pub tables_created_in_change: &'a HashSet<String>,

    /// Whether this migration unit runs in a transaction.
    pub run_in_transaction: bool,

    /// Whether this is a down/rollback migration.
    pub is_down: bool,

    /// The source file being linted.
    pub file: &'a Path,
}

impl<'a> LintContext<'a> {
    /// Check if a table existed before this change and was not created in the
    /// current set of changed files.
    pub fn is_existing_table(&self, table_key: &str) -> bool {
        self.catalog_before.has_table(table_key)
            && !self.tables_created_in_change.contains(table_key)
    }

    /// Check if a table matches the given scope filter.
    pub fn table_matches_scope(&self, table_key: &str, scope: TableScope) -> bool {
        match scope {
            TableScope::ExcludeCreatedInChange => self.is_existing_table(table_key),
            TableScope::AnyPreExisting => self.catalog_before.has_table(table_key),
        }
    }
}

/// Trait that every rule implements.
pub trait Rule: Send + Sync {
    /// Stable rule identifier.
    fn id(&self) -> RuleId;

    /// Default severity for this rule.
    fn default_severity(&self) -> Severity;

    /// Human-readable short description.
    fn description(&self) -> &'static str;

    /// Detailed explanation for --explain. Includes failure mode, example, fix.
    fn explain(&self) -> &'static str;

    /// Run the rule against a single migration unit.
    ///
    /// `statements` are the IR nodes for the unit being linted.
    /// `ctx` provides catalog state and changed-file context.
    ///
    /// Returns findings, typically with severity from `default_severity()`.
    /// Some rules (e.g. PGM007, PGM009) may use per-finding severity.
    /// The caller handles down-migration severity capping and suppression filtering.
    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding>;

    /// Convenience method to construct a Finding with this rule's ID and default severity.
    fn make_finding(&self, message: String, file: &std::path::Path, span: &SourceSpan) -> Finding {
        Finding::new(self.id(), self.default_severity(), message, file, span)
    }
}

/// Cap all finding severities to INFO for down/rollback migrations (PGM901).
///
/// Down migrations are informational only. This function mutates the
/// findings in place, setting every severity to `Severity::Info`.
pub fn cap_for_down_migration(findings: &mut [Finding]) {
    for f in findings {
        f.severity = Severity::Info;
    }
}

/// Registry of all rules.
pub struct RuleRegistry {
    rules: Vec<Box<dyn Rule>>,
}

impl RuleRegistry {
    /// Create a new empty rule registry.
    pub fn new() -> Self {
        Self { rules: vec![] }
    }

    /// Register all built-in rules.
    pub fn register_defaults(&mut self) {
        MigrationRule::iter()
            .map(RuleId::Migration)
            .chain(TypeChoiceRule::iter().map(RuleId::TypeChoice))
            .for_each(|r| self.register(r.into()));
    }

    /// Register a single rule.
    pub fn register(&mut self, rule: Box<dyn Rule>) {
        self.rules.push(rule);
    }

    /// Get a rule by string ID (for --explain and config validation).
    pub fn get(&self, id: &RuleId) -> Option<&dyn Rule> {
        self.get_by_id(*id)
    }

    /// Get a rule by typed ID.
    pub fn get_by_id(&self, id: RuleId) -> Option<&dyn Rule> {
        self.rules.iter().find(|r| r.id() == id).map(|b| &**b)
    }

    /// Iterate all rules.
    pub fn iter(&self) -> impl Iterator<Item = &dyn Rule> {
        self.rules.iter().map(|b| &**b)
    }
}

impl Default for RuleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cap_for_down_migration() {
        let mut findings = vec![
            Finding {
                rule_id: RuleId::Migration(MigrationRule::Pgm001),
                severity: Severity::Critical,
                message: "test".to_string(),
                file: PathBuf::from("test.sql"),
                start_line: 1,
                end_line: 1,
            },
            Finding {
                rule_id: RuleId::Migration(MigrationRule::Pgm004),
                severity: Severity::Major,
                message: "test".to_string(),
                file: PathBuf::from("test.sql"),
                start_line: 2,
                end_line: 2,
            },
        ];

        cap_for_down_migration(&mut findings);

        assert_eq!(findings[0].severity, Severity::Info);
        assert_eq!(findings[1].severity, Severity::Info);
    }

    #[test]
    fn test_severity_ordering() {
        assert!(Severity::Info < Severity::Minor);
        assert!(Severity::Minor < Severity::Major);
        assert!(Severity::Major < Severity::Critical);
        assert!(Severity::Critical < Severity::Blocker);
    }

    #[test]
    fn test_all_rules_have_valid_description() {
        let mut registry = RuleRegistry::new();
        registry.register_defaults();

        for rule in registry.iter() {
            let id = rule.id();
            let desc = rule.description();
            assert!(desc.len() > 10, "{id} description too short: {desc:?}");
        }
    }

    #[test]
    fn test_all_rules_have_valid_explain() {
        let mut registry = RuleRegistry::new();
        registry.register_defaults();

        for rule in registry.iter() {
            let id = rule.id();
            let explain = rule.explain();
            assert!(
                explain.len() > 20,
                "{id} explain text too short: {explain:?}"
            );
            assert!(
                explain.contains(id.as_str()),
                "{id} explain text should reference its own rule ID"
            );
        }
    }

    #[test]
    fn test_severity_parse() {
        assert_eq!(Severity::parse("blocker"), Some(Severity::Blocker));
        assert_eq!(Severity::parse("critical"), Some(Severity::Critical));
        assert_eq!(Severity::parse("major"), Some(Severity::Major));
        assert_eq!(Severity::parse("minor"), Some(Severity::Minor));
        assert_eq!(Severity::parse("info"), Some(Severity::Info));
        // Case-insensitive
        assert_eq!(Severity::parse("CRITICAL"), Some(Severity::Critical));
        assert_eq!(Severity::parse("Blocker"), Some(Severity::Blocker));
        // Invalid
        assert_eq!(Severity::parse("garbage"), None);
        assert_eq!(Severity::parse("none"), None);
    }

    // -----------------------------------------------------------------------
    // RuleId enum tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_rule_id_display_round_trip() {
        // Every variant should survive Display → FromStr round-trip
        let all: Vec<RuleId> = MigrationRule::iter()
            .map(RuleId::Migration)
            .chain(TypeChoiceRule::iter().map(RuleId::TypeChoice))
            .chain(MetaRule::iter().map(RuleId::Meta))
            .collect();
        for id in &all {
            let s = id.to_string();
            let parsed: RuleId = s.parse().unwrap_or_else(|_| panic!("failed to parse {s}"));
            assert_eq!(*id, parsed, "round-trip failed for {s}");
            assert_eq!(id.as_str(), s.as_str());
        }
        // 27 registered rules + 1 meta = 28
        assert_eq!(all.len(), 28);
    }

    #[test]
    fn test_rule_id_from_str_unknown() {
        assert!("PGM000".parse::<RuleId>().is_err());
        assert!("PGM008".parse::<RuleId>().is_err());
        assert!("PGM999".parse::<RuleId>().is_err());
        assert!("garbage".parse::<RuleId>().is_err());
        assert!("pgm001".parse::<RuleId>().is_err()); // case-sensitive
    }

    #[test]
    fn test_rule_id_ordering() {
        // Migration < TypeChoice < Meta (by derive Ord on enum variant order)
        assert!(
            RuleId::Migration(MigrationRule::Pgm022) < RuleId::TypeChoice(TypeChoiceRule::Pgm101)
        );
        assert!(RuleId::TypeChoice(TypeChoiceRule::Pgm108) < RuleId::Meta(MetaRule::Pgm901));
        // Within Migration family
        assert!(
            RuleId::Migration(MigrationRule::Pgm001) < RuleId::Migration(MigrationRule::Pgm022)
        );
    }

    #[test]
    fn test_rule_id_serialize_json() {
        let id = RuleId::Migration(MigrationRule::Pgm003);
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, "\"PGM003\"");
    }

    #[test]
    fn test_parse_rule_id_error_display() {
        let err = "BOGUS".parse::<RuleId>().unwrap_err();
        assert_eq!(err.to_string(), "unknown rule ID: 'BOGUS'");
    }
}
