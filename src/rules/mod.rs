//! Rule engine and lint context
//!
//! Each rule implements the `Rule` trait and checks for specific migration safety issues.
//! Rules receive IR nodes and catalog state, returning findings with severity levels.

use serde::Serialize;
use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use strum::IntoEnumIterator;
use strum_macros::{EnumIter, EnumString, IntoStaticStr};

use crate::catalog::Catalog;
use crate::parser::ir::{IrNode, Located, SourceSpan};

#[cfg(test)]
pub mod test_helpers;

pub mod alter_table_check;
pub mod column_type_check;
pub mod drop_column_check;
pub mod existing_table_check;

// 0xx — Unsafe DDL
mod pgm001;
mod pgm002;
mod pgm003;
mod pgm004;
mod pgm005;
mod pgm006;
mod pgm007;
mod pgm008;
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

// 1xx — Type anti-patterns
mod pgm101;
mod pgm102;
mod pgm103;
mod pgm104;
mod pgm105;
mod pgm106;

// 2xx — Destructive operations
mod pgm201;
mod pgm202;
mod pgm203;
mod pgm204;
mod pgm205;

// 3xx — DML in migrations
mod pgm301;
mod pgm302;
mod pgm303;

// 4xx — Idempotency guards
mod pgm401;
mod pgm402;
mod pgm403;

// 5xx — Schema design & informational
mod pgm501;
mod pgm502;
mod pgm503;
mod pgm504;
mod pgm505;
mod pgm506;

// ---------------------------------------------------------------------------
// Rule ID enum
// ---------------------------------------------------------------------------

/// Strongly-typed rule identifier.
///
/// A flat enum covering all rule families. Match statements are exhaustive:
/// adding a new variant forces updates in `sonarqube_meta()`, `effort_minutes()`,
/// and everywhere else a rule ID is dispatched on.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, EnumIter, EnumString, IntoStaticStr,
)]
pub enum RuleId {
    // 0xx — Unsafe DDL
    /// `CREATE INDEX` without `CONCURRENTLY` on existing tables.
    #[strum(serialize = "PGM001")]
    Pgm001,
    /// `DROP INDEX` without `CONCURRENTLY` on existing tables.
    #[strum(serialize = "PGM002")]
    Pgm002,
    /// Concurrent index operations inside a transaction.
    #[strum(serialize = "PGM003")]
    Pgm003,
    /// `DETACH PARTITION` without `CONCURRENTLY` on existing tables.
    #[strum(serialize = "PGM004")]
    Pgm004,
    /// `ATTACH PARTITION` of existing table without pre-validated `CHECK`.
    #[strum(serialize = "PGM005")]
    Pgm005,
    /// Volatile function defaults on columns.
    #[strum(serialize = "PGM006")]
    Pgm006,
    /// Column type changes on existing tables.
    #[strum(serialize = "PGM007")]
    Pgm007,
    /// Adding a `NOT NULL` column without a `DEFAULT` to an existing table.
    #[strum(serialize = "PGM008")]
    Pgm008,
    /// Dropping a column from an existing table.
    #[strum(serialize = "PGM009")]
    Pgm009,
    /// Dropping a column that participates in a unique constraint or unique index.
    #[strum(serialize = "PGM010")]
    Pgm010,
    /// Dropping a column that participates in the table's primary key.
    #[strum(serialize = "PGM011")]
    Pgm011,
    /// Dropping a column that participates in a foreign key constraint.
    #[strum(serialize = "PGM012")]
    Pgm012,
    /// `SET NOT NULL` on an existing table column.
    #[strum(serialize = "PGM013")]
    Pgm013,
    /// Adding a `FOREIGN KEY` without `NOT VALID` on an existing table.
    #[strum(serialize = "PGM014")]
    Pgm014,
    /// Adding a `CHECK` constraint without `NOT VALID` on an existing table.
    #[strum(serialize = "PGM015")]
    Pgm015,
    /// Adding a `PRIMARY KEY` without a prior unique index.
    #[strum(serialize = "PGM016")]
    Pgm016,
    /// Adding a `UNIQUE` constraint without a pre-existing unique index.
    #[strum(serialize = "PGM017")]
    Pgm017,
    /// `CLUSTER` on an existing table (ACCESS EXCLUSIVE lock for full rewrite).
    #[strum(serialize = "PGM018")]
    Pgm018,
    /// `ADD EXCLUDE` constraint on an existing table (ACCESS EXCLUSIVE lock, no online path).
    #[strum(serialize = "PGM019")]
    Pgm019,
    /// `DISABLE TRIGGER` on a table (suppresses FK enforcement).
    #[strum(serialize = "PGM020")]
    Pgm020,

    // 1xx — Type anti-patterns
    /// `timestamp` without time zone.
    #[strum(serialize = "PGM101")]
    Pgm101,
    /// `timestamp(0)` or `timestamptz(0)`.
    #[strum(serialize = "PGM102")]
    Pgm102,
    /// `char(n)`.
    #[strum(serialize = "PGM103")]
    Pgm103,
    /// `money` type.
    #[strum(serialize = "PGM104")]
    Pgm104,
    /// `serial` / `bigserial` / `smallserial` column types.
    #[strum(serialize = "PGM105")]
    Pgm105,
    /// `json` type (use `jsonb` instead).
    #[strum(serialize = "PGM106")]
    Pgm106,

    // 2xx — Destructive operations
    /// Dropping an existing table.
    #[strum(serialize = "PGM201")]
    Pgm201,
    /// `DROP TABLE CASCADE` on existing table.
    #[strum(serialize = "PGM202")]
    Pgm202,
    /// `TRUNCATE TABLE` on existing table.
    #[strum(serialize = "PGM203")]
    Pgm203,
    /// `TRUNCATE TABLE CASCADE` on existing table.
    #[strum(serialize = "PGM204")]
    Pgm204,
    /// `DROP SCHEMA CASCADE`.
    #[strum(serialize = "PGM205")]
    Pgm205,

    // 3xx — DML in migrations
    /// `INSERT INTO` existing table in migration.
    #[strum(serialize = "PGM301")]
    Pgm301,
    /// `UPDATE` on existing table in migration.
    #[strum(serialize = "PGM302")]
    Pgm302,
    /// `DELETE FROM` existing table in migration.
    #[strum(serialize = "PGM303")]
    Pgm303,

    // 4xx — Idempotency guards
    /// Missing `IF EXISTS` on `DROP TABLE` / `DROP INDEX`.
    #[strum(serialize = "PGM401")]
    Pgm401,
    /// Missing `IF NOT EXISTS` on `CREATE TABLE` / `CREATE INDEX`.
    #[strum(serialize = "PGM402")]
    Pgm402,
    /// `CREATE TABLE IF NOT EXISTS` for already-existing table (misleading no-op).
    #[strum(serialize = "PGM403")]
    Pgm403,

    // 5xx — Schema design & informational
    /// Foreign keys without a covering index on the referencing table.
    #[strum(serialize = "PGM501")]
    Pgm501,
    /// Tables created without a primary key.
    #[strum(serialize = "PGM502")]
    Pgm502,
    /// Tables using `UNIQUE NOT NULL` instead of a proper `PRIMARY KEY`.
    #[strum(serialize = "PGM503")]
    Pgm503,
    /// `ALTER TABLE ... RENAME TO` on existing tables.
    #[strum(serialize = "PGM504")]
    Pgm504,
    /// `RENAME COLUMN` on an existing table.
    #[strum(serialize = "PGM505")]
    Pgm505,
    /// `CREATE UNLOGGED TABLE`.
    #[strum(serialize = "PGM506")]
    Pgm506,

    // 9xx — Meta-behavior
    /// Down-migration severity capping (not a standalone rule).
    #[strum(serialize = "PGM901")]
    Pgm901,
}

impl RuleId {
    /// Zero-allocation string representation.
    ///
    /// Delegates to the [`IntoStaticStr`] derive which maps each variant to
    /// its `#[strum(serialize = "…")]` string.
    pub fn as_str(&self) -> &'static str {
        self.into()
    }

    /// Whether this is a meta-behavior rule (not a standalone lint rule).
    pub fn is_meta(&self) -> bool {
        matches!(self, Self::Pgm901)
    }

    /// Iterator over all non-meta rule IDs (rules that produce findings).
    pub fn lint_rules() -> impl Iterator<Item = Self> {
        Self::iter().filter(|r| !r.is_meta())
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

// `FromStr` is derived via `EnumString` — strum generates a match from
// `#[strum(serialize = "…")]` attributes. `Err` type is `strum::ParseError`.

impl Rule for RuleId {
    fn id(&self) -> Self {
        *self
    }

    fn default_severity(&self) -> Severity {
        match self {
            // 0xx — Unsafe DDL
            Self::Pgm001
            | Self::Pgm002
            | Self::Pgm003
            | Self::Pgm004
            | Self::Pgm007
            | Self::Pgm008
            | Self::Pgm013
            | Self::Pgm014
            | Self::Pgm015
            | Self::Pgm017
            | Self::Pgm018
            | Self::Pgm019 => Severity::Critical,
            Self::Pgm005 | Self::Pgm011 | Self::Pgm016 => Severity::Major,
            Self::Pgm006 | Self::Pgm010 | Self::Pgm012 | Self::Pgm020 => Severity::Minor,
            Self::Pgm009 => Severity::Info,

            // 1xx — Type anti-patterns
            Self::Pgm101 | Self::Pgm102 | Self::Pgm103 | Self::Pgm104 | Self::Pgm106 => {
                Severity::Minor
            }
            Self::Pgm105 => Severity::Info,

            // 2xx — Destructive operations
            Self::Pgm201 | Self::Pgm203 => Severity::Minor,
            Self::Pgm202 | Self::Pgm204 => Severity::Major,
            Self::Pgm205 => Severity::Critical,

            // 3xx — DML in migrations
            Self::Pgm301 => Severity::Info,
            Self::Pgm302 | Self::Pgm303 => Severity::Minor,

            // 4xx — Idempotency guards
            Self::Pgm401 | Self::Pgm402 | Self::Pgm403 => Severity::Minor,

            // 5xx — Schema design
            Self::Pgm501 | Self::Pgm502 => Severity::Major,
            Self::Pgm503 | Self::Pgm504 | Self::Pgm505 | Self::Pgm506 => Severity::Info,

            // 9xx — Meta
            Self::Pgm901 => Severity::Info,
        }
    }

    fn description(&self) -> &'static str {
        match self {
            Self::Pgm001 => pgm001::DESCRIPTION,
            Self::Pgm002 => pgm002::DESCRIPTION,
            Self::Pgm003 => pgm003::DESCRIPTION,
            Self::Pgm004 => pgm004::DESCRIPTION,
            Self::Pgm005 => pgm005::DESCRIPTION,
            Self::Pgm006 => pgm006::DESCRIPTION,
            Self::Pgm007 => pgm007::DESCRIPTION,
            Self::Pgm008 => pgm008::DESCRIPTION,
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
            Self::Pgm101 => pgm101::DESCRIPTION,
            Self::Pgm102 => pgm102::DESCRIPTION,
            Self::Pgm103 => pgm103::DESCRIPTION,
            Self::Pgm104 => pgm104::DESCRIPTION,
            Self::Pgm105 => pgm105::DESCRIPTION,
            Self::Pgm106 => pgm106::DESCRIPTION,
            Self::Pgm201 => pgm201::DESCRIPTION,
            Self::Pgm202 => pgm202::DESCRIPTION,
            Self::Pgm203 => pgm203::DESCRIPTION,
            Self::Pgm204 => pgm204::DESCRIPTION,
            Self::Pgm205 => pgm205::DESCRIPTION,
            Self::Pgm301 => pgm301::DESCRIPTION,
            Self::Pgm302 => pgm302::DESCRIPTION,
            Self::Pgm303 => pgm303::DESCRIPTION,
            Self::Pgm401 => pgm401::DESCRIPTION,
            Self::Pgm402 => pgm402::DESCRIPTION,
            Self::Pgm403 => pgm403::DESCRIPTION,
            Self::Pgm501 => pgm501::DESCRIPTION,
            Self::Pgm502 => pgm502::DESCRIPTION,
            Self::Pgm503 => pgm503::DESCRIPTION,
            Self::Pgm504 => pgm504::DESCRIPTION,
            Self::Pgm505 => pgm505::DESCRIPTION,
            Self::Pgm506 => pgm506::DESCRIPTION,
            Self::Pgm901 => {
                "Meta rules alter the behavior of other rules, they are not rules themselves"
            }
        }
    }

    fn explain(&self) -> &'static str {
        match self {
            Self::Pgm001 => pgm001::EXPLAIN,
            Self::Pgm002 => pgm002::EXPLAIN,
            Self::Pgm003 => pgm003::EXPLAIN,
            Self::Pgm004 => pgm004::EXPLAIN,
            Self::Pgm005 => pgm005::EXPLAIN,
            Self::Pgm006 => pgm006::EXPLAIN,
            Self::Pgm007 => pgm007::EXPLAIN,
            Self::Pgm008 => pgm008::EXPLAIN,
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
            Self::Pgm101 => pgm101::EXPLAIN,
            Self::Pgm102 => pgm102::EXPLAIN,
            Self::Pgm103 => pgm103::EXPLAIN,
            Self::Pgm104 => pgm104::EXPLAIN,
            Self::Pgm105 => pgm105::EXPLAIN,
            Self::Pgm106 => pgm106::EXPLAIN,
            Self::Pgm201 => pgm201::EXPLAIN,
            Self::Pgm202 => pgm202::EXPLAIN,
            Self::Pgm203 => pgm203::EXPLAIN,
            Self::Pgm204 => pgm204::EXPLAIN,
            Self::Pgm205 => pgm205::EXPLAIN,
            Self::Pgm301 => pgm301::EXPLAIN,
            Self::Pgm302 => pgm302::EXPLAIN,
            Self::Pgm303 => pgm303::EXPLAIN,
            Self::Pgm401 => pgm401::EXPLAIN,
            Self::Pgm402 => pgm402::EXPLAIN,
            Self::Pgm403 => pgm403::EXPLAIN,
            Self::Pgm501 => pgm501::EXPLAIN,
            Self::Pgm502 => pgm502::EXPLAIN,
            Self::Pgm503 => pgm503::EXPLAIN,
            Self::Pgm504 => pgm504::EXPLAIN,
            Self::Pgm505 => pgm505::EXPLAIN,
            Self::Pgm506 => pgm506::EXPLAIN,
            Self::Pgm901 => "This rule caps severity of triggered rules to INFO (not in SonarQube)",
        }
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        match self {
            Self::Pgm001 => pgm001::check(*self, statements, ctx),
            Self::Pgm002 => pgm002::check(*self, statements, ctx),
            Self::Pgm003 => pgm003::check(*self, statements, ctx),
            Self::Pgm004 => pgm004::check(*self, statements, ctx),
            Self::Pgm005 => pgm005::check(*self, statements, ctx),
            Self::Pgm006 => pgm006::check(*self, statements, ctx),
            Self::Pgm007 => pgm007::check(*self, statements, ctx),
            Self::Pgm008 => pgm008::check(*self, statements, ctx),
            Self::Pgm009 => pgm009::check(*self, statements, ctx),
            Self::Pgm010 => pgm010::check(*self, statements, ctx),
            Self::Pgm011 => pgm011::check(*self, statements, ctx),
            Self::Pgm012 => pgm012::check(*self, statements, ctx),
            Self::Pgm013 => pgm013::check(*self, statements, ctx),
            Self::Pgm014 => pgm014::check(*self, statements, ctx),
            Self::Pgm015 => pgm015::check(*self, statements, ctx),
            Self::Pgm016 => pgm016::check(*self, statements, ctx),
            Self::Pgm017 => pgm017::check(*self, statements, ctx),
            Self::Pgm018 => pgm018::check(*self, statements, ctx),
            Self::Pgm019 => pgm019::check(*self, statements, ctx),
            Self::Pgm020 => pgm020::check(*self, statements, ctx),
            Self::Pgm101 => pgm101::check(*self, statements, ctx),
            Self::Pgm102 => pgm102::check(*self, statements, ctx),
            Self::Pgm103 => pgm103::check(*self, statements, ctx),
            Self::Pgm104 => pgm104::check(*self, statements, ctx),
            Self::Pgm105 => pgm105::check(*self, statements, ctx),
            Self::Pgm106 => pgm106::check(*self, statements, ctx),
            Self::Pgm201 => pgm201::check(*self, statements, ctx),
            Self::Pgm202 => pgm202::check(*self, statements, ctx),
            Self::Pgm203 => pgm203::check(*self, statements, ctx),
            Self::Pgm204 => pgm204::check(*self, statements, ctx),
            Self::Pgm205 => pgm205::check(*self, statements, ctx),
            Self::Pgm301 => pgm301::check(*self, statements, ctx),
            Self::Pgm302 => pgm302::check(*self, statements, ctx),
            Self::Pgm303 => pgm303::check(*self, statements, ctx),
            Self::Pgm401 => pgm401::check(*self, statements, ctx),
            Self::Pgm402 => pgm402::check(*self, statements, ctx),
            Self::Pgm403 => pgm403::check(*self, statements, ctx),
            Self::Pgm501 => pgm501::check(*self, statements, ctx),
            Self::Pgm502 => pgm502::check(*self, statements, ctx),
            Self::Pgm503 => pgm503::check(*self, statements, ctx),
            Self::Pgm504 => pgm504::check(*self, statements, ctx),
            Self::Pgm505 => pgm505::check(*self, statements, ctx),
            Self::Pgm506 => pgm506::check(*self, statements, ctx),
            Self::Pgm901 => vec![],
        }
    }
}

// ---------------------------------------------------------------------------
// Severity
// ---------------------------------------------------------------------------

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

    /// Title-case severity string for documentation output.
    pub fn title_case(&self) -> &'static str {
        match self {
            Severity::Info => "Info",
            Severity::Minor => "Minor",
            Severity::Major => "Major",
            Severity::Critical => "Critical",
            Severity::Blocker => "Blocker",
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
    /// Used for post-file checks (PGM501, PGM502, PGM503).
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

    /// Check whether a partition child should be exempt from PK-related rules
    /// (PGM502, PGM503) because its parent has a PK or the parent is not in
    /// the catalog.
    ///
    /// Checks two sources:
    /// 1. The IR's `partition_of` field (`CREATE TABLE child PARTITION OF parent`).
    /// 2. The catalog's `parent_table` field (set by `ALTER TABLE parent ATTACH PARTITION child`).
    ///
    /// Returns `true` (suppress) when:
    /// - Parent has a PK in `catalog_after` — PK is inherited by the child.
    /// - Parent is not in `catalog_after` — trust that production parents have a PK
    ///   (common in incremental CI where only new migrations are analyzed).
    ///
    /// Returns `false` (fire normally) when:
    /// - The table is not a partition child.
    /// - The parent exists but lacks a PK.
    pub fn partition_child_inherits_pk(
        &self,
        ir_partition_of: Option<&crate::parser::ir::QualifiedName>,
        table_key: &str,
    ) -> bool {
        // Primary: from IR (CREATE TABLE ... PARTITION OF parent)
        if let Some(parent_name) = ir_partition_of {
            let parent_key = parent_name.catalog_key();
            return match self.catalog_after.get_table(parent_key) {
                Some(parent) if parent.has_primary_key => true,
                Some(_) => false,
                None => true,
            };
        }

        // Fallback: from catalog (ALTER TABLE parent ATTACH PARTITION child)
        if let Some(table) = self.catalog_after.get_table(table_key)
            && let Some(ref parent_key) = table.parent_table
        {
            return match self.catalog_after.get_table(parent_key) {
                Some(parent) if parent.has_primary_key => true,
                Some(_) => false,
                None => true,
            };
        }

        false
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
    /// Some rules (e.g. PGM006, PGM007) may use per-finding severity.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cap_for_down_migration() {
        let mut findings = vec![
            Finding {
                rule_id: RuleId::Pgm001,
                severity: Severity::Critical,
                message: "test".to_string(),
                file: PathBuf::from("test.sql"),
                start_line: 1,
                end_line: 1,
            },
            Finding {
                rule_id: RuleId::Pgm502,
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
        for id in RuleId::lint_rules() {
            let desc = id.description();
            assert!(desc.len() > 10, "{id} description too short: {desc:?}");
        }
    }

    #[test]
    fn test_all_rules_have_valid_explain() {
        for id in RuleId::lint_rules() {
            let explain = id.explain();
            assert!(
                explain.len() > 20,
                "{id} explain text too short: {explain:?}"
            );
        }
    }

    #[test]
    fn test_explain_output_snapshots() {
        for id in RuleId::lint_rules() {
            let output = format!(
                "Rule: {}\nSeverity: {}\nDescription: {}\n\n{}",
                id,
                id.default_severity(),
                id.description(),
                id.explain()
            );
            insta::assert_snapshot!(format!("explain_{}", id), output);
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
        for id in RuleId::iter() {
            let s = id.to_string();
            let parsed: RuleId = s.parse().unwrap_or_else(|_| panic!("failed to parse {s}"));
            assert_eq!(id, parsed, "round-trip failed for {s}");
            assert_eq!(id.as_str(), s.as_str());
        }
        assert_eq!(RuleId::iter().count(), 44);
    }

    #[test]
    fn test_rule_id_from_str_unknown() {
        assert!("PGM000".parse::<RuleId>().is_err());
        assert!("PGM999".parse::<RuleId>().is_err());
        assert!("garbage".parse::<RuleId>().is_err());
        assert!("pgm001".parse::<RuleId>().is_err()); // case-sensitive
    }

    #[test]
    fn test_rule_id_ordering() {
        // Variants are ordered by declaration order
        assert!(RuleId::Pgm017 < RuleId::Pgm101);
        assert!(RuleId::Pgm106 < RuleId::Pgm201);
        assert!(RuleId::Pgm201 < RuleId::Pgm301);
        assert!(RuleId::Pgm303 < RuleId::Pgm401);
        assert!(RuleId::Pgm402 < RuleId::Pgm501);
        assert!(RuleId::Pgm506 < RuleId::Pgm901);
        // Within a family
        assert!(RuleId::Pgm001 < RuleId::Pgm017);
    }

    #[test]
    fn test_rule_id_serialize_json() {
        let id = RuleId::Pgm003;
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, "\"PGM003\"");
    }

    #[test]
    fn test_parse_rule_id_error_display() {
        let err = "BOGUS".parse::<RuleId>().unwrap_err();
        assert_eq!(err.to_string(), "Matching variant not found");
    }

    #[test]
    fn meta_rule_pgm901_description_is_non_empty() {
        let rule_id = RuleId::Pgm901;
        let desc = rule_id.description();
        assert!(!desc.is_empty(), "PGM901 description should not be empty");
        assert!(
            desc.contains("Meta"),
            "PGM901 description should mention Meta"
        );
    }

    #[test]
    fn meta_rule_pgm901_explain_is_non_empty() {
        let rule_id = RuleId::Pgm901;
        let explain = rule_id.explain();
        assert!(!explain.is_empty(), "PGM901 explain should not be empty");
        assert!(
            explain.contains("INFO"),
            "PGM901 explain should mention INFO"
        );
    }
}
