use serde::Serialize;
use strum::IntoEnumIterator;
use strum_macros::{EnumIter, EnumString, IntoStaticStr};

use crate::{
    Finding, IrNode, Located, Rule,
    rules::{LintContext, severity::Severity},
};

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
    /// `DROP NOT NULL` on an existing table allows NULL values.
    #[strum(serialize = "PGM507")]
    Pgm507,

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

impl std::fmt::Display for RuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
            Self::Pgm503 | Self::Pgm504 | Self::Pgm505 | Self::Pgm506 | Self::Pgm507 => {
                Severity::Info
            }

            // 9xx — Meta
            Self::Pgm901 => Severity::Info,
        }
    }

    fn description(&self) -> &'static str {
        match self {
            Self::Pgm001 => super::pgm001::DESCRIPTION,
            Self::Pgm002 => super::pgm002::DESCRIPTION,
            Self::Pgm003 => super::pgm003::DESCRIPTION,
            Self::Pgm004 => super::pgm004::DESCRIPTION,
            Self::Pgm005 => super::pgm005::DESCRIPTION,
            Self::Pgm006 => super::pgm006::DESCRIPTION,
            Self::Pgm007 => super::pgm007::DESCRIPTION,
            Self::Pgm008 => super::pgm008::DESCRIPTION,
            Self::Pgm009 => super::pgm009::DESCRIPTION,
            Self::Pgm010 => super::pgm010::DESCRIPTION,
            Self::Pgm011 => super::pgm011::DESCRIPTION,
            Self::Pgm012 => super::pgm012::DESCRIPTION,
            Self::Pgm013 => super::pgm013::DESCRIPTION,
            Self::Pgm014 => super::pgm014::DESCRIPTION,
            Self::Pgm015 => super::pgm015::DESCRIPTION,
            Self::Pgm016 => super::pgm016::DESCRIPTION,
            Self::Pgm017 => super::pgm017::DESCRIPTION,
            Self::Pgm018 => super::pgm018::DESCRIPTION,
            Self::Pgm019 => super::pgm019::DESCRIPTION,
            Self::Pgm020 => super::pgm020::DESCRIPTION,
            Self::Pgm507 => super::pgm507::DESCRIPTION,
            Self::Pgm101 => super::pgm101::DESCRIPTION,
            Self::Pgm102 => super::pgm102::DESCRIPTION,
            Self::Pgm103 => super::pgm103::DESCRIPTION,
            Self::Pgm104 => super::pgm104::DESCRIPTION,
            Self::Pgm105 => super::pgm105::DESCRIPTION,
            Self::Pgm106 => super::pgm106::DESCRIPTION,
            Self::Pgm201 => super::pgm201::DESCRIPTION,
            Self::Pgm202 => super::pgm202::DESCRIPTION,
            Self::Pgm203 => super::pgm203::DESCRIPTION,
            Self::Pgm204 => super::pgm204::DESCRIPTION,
            Self::Pgm205 => super::pgm205::DESCRIPTION,
            Self::Pgm301 => super::pgm301::DESCRIPTION,
            Self::Pgm302 => super::pgm302::DESCRIPTION,
            Self::Pgm303 => super::pgm303::DESCRIPTION,
            Self::Pgm401 => super::pgm401::DESCRIPTION,
            Self::Pgm402 => super::pgm402::DESCRIPTION,
            Self::Pgm403 => super::pgm403::DESCRIPTION,
            Self::Pgm501 => super::pgm501::DESCRIPTION,
            Self::Pgm502 => super::pgm502::DESCRIPTION,
            Self::Pgm503 => super::pgm503::DESCRIPTION,
            Self::Pgm504 => super::pgm504::DESCRIPTION,
            Self::Pgm505 => super::pgm505::DESCRIPTION,
            Self::Pgm506 => super::pgm506::DESCRIPTION,
            Self::Pgm901 => {
                "Meta rules alter the behavior of other rules, they are not rules themselves"
            }
        }
    }

    fn explain(&self) -> &'static str {
        match self {
            Self::Pgm001 => super::pgm001::EXPLAIN,
            Self::Pgm002 => super::pgm002::EXPLAIN,
            Self::Pgm003 => super::pgm003::EXPLAIN,
            Self::Pgm004 => super::pgm004::EXPLAIN,
            Self::Pgm005 => super::pgm005::EXPLAIN,
            Self::Pgm006 => super::pgm006::EXPLAIN,
            Self::Pgm007 => super::pgm007::EXPLAIN,
            Self::Pgm008 => super::pgm008::EXPLAIN,
            Self::Pgm009 => super::pgm009::EXPLAIN,
            Self::Pgm010 => super::pgm010::EXPLAIN,
            Self::Pgm011 => super::pgm011::EXPLAIN,
            Self::Pgm012 => super::pgm012::EXPLAIN,
            Self::Pgm013 => super::pgm013::EXPLAIN,
            Self::Pgm014 => super::pgm014::EXPLAIN,
            Self::Pgm015 => super::pgm015::EXPLAIN,
            Self::Pgm016 => super::pgm016::EXPLAIN,
            Self::Pgm017 => super::pgm017::EXPLAIN,
            Self::Pgm018 => super::pgm018::EXPLAIN,
            Self::Pgm019 => super::pgm019::EXPLAIN,
            Self::Pgm020 => super::pgm020::EXPLAIN,
            Self::Pgm507 => super::pgm507::EXPLAIN,
            Self::Pgm101 => super::pgm101::EXPLAIN,
            Self::Pgm102 => super::pgm102::EXPLAIN,
            Self::Pgm103 => super::pgm103::EXPLAIN,
            Self::Pgm104 => super::pgm104::EXPLAIN,
            Self::Pgm105 => super::pgm105::EXPLAIN,
            Self::Pgm106 => super::pgm106::EXPLAIN,
            Self::Pgm201 => super::pgm201::EXPLAIN,
            Self::Pgm202 => super::pgm202::EXPLAIN,
            Self::Pgm203 => super::pgm203::EXPLAIN,
            Self::Pgm204 => super::pgm204::EXPLAIN,
            Self::Pgm205 => super::pgm205::EXPLAIN,
            Self::Pgm301 => super::pgm301::EXPLAIN,
            Self::Pgm302 => super::pgm302::EXPLAIN,
            Self::Pgm303 => super::pgm303::EXPLAIN,
            Self::Pgm401 => super::pgm401::EXPLAIN,
            Self::Pgm402 => super::pgm402::EXPLAIN,
            Self::Pgm403 => super::pgm403::EXPLAIN,
            Self::Pgm501 => super::pgm501::EXPLAIN,
            Self::Pgm502 => super::pgm502::EXPLAIN,
            Self::Pgm503 => super::pgm503::EXPLAIN,
            Self::Pgm504 => super::pgm504::EXPLAIN,
            Self::Pgm505 => super::pgm505::EXPLAIN,
            Self::Pgm506 => super::pgm506::EXPLAIN,
            Self::Pgm901 => "This rule caps severity of triggered rules to INFO (not in SonarQube)",
        }
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        match self {
            Self::Pgm001 => super::pgm001::check(*self, statements, ctx),
            Self::Pgm002 => super::pgm002::check(*self, statements, ctx),
            Self::Pgm003 => super::pgm003::check(*self, statements, ctx),
            Self::Pgm004 => super::pgm004::check(*self, statements, ctx),
            Self::Pgm005 => super::pgm005::check(*self, statements, ctx),
            Self::Pgm006 => super::pgm006::check(*self, statements, ctx),
            Self::Pgm007 => super::pgm007::check(*self, statements, ctx),
            Self::Pgm008 => super::pgm008::check(*self, statements, ctx),
            Self::Pgm009 => super::pgm009::check(*self, statements, ctx),
            Self::Pgm010 => super::pgm010::check(*self, statements, ctx),
            Self::Pgm011 => super::pgm011::check(*self, statements, ctx),
            Self::Pgm012 => super::pgm012::check(*self, statements, ctx),
            Self::Pgm013 => super::pgm013::check(*self, statements, ctx),
            Self::Pgm014 => super::pgm014::check(*self, statements, ctx),
            Self::Pgm015 => super::pgm015::check(*self, statements, ctx),
            Self::Pgm016 => super::pgm016::check(*self, statements, ctx),
            Self::Pgm017 => super::pgm017::check(*self, statements, ctx),
            Self::Pgm018 => super::pgm018::check(*self, statements, ctx),
            Self::Pgm019 => super::pgm019::check(*self, statements, ctx),
            Self::Pgm020 => super::pgm020::check(*self, statements, ctx),
            Self::Pgm507 => super::pgm507::check(*self, statements, ctx),
            Self::Pgm101 => super::pgm101::check(*self, statements, ctx),
            Self::Pgm102 => super::pgm102::check(*self, statements, ctx),
            Self::Pgm103 => super::pgm103::check(*self, statements, ctx),
            Self::Pgm104 => super::pgm104::check(*self, statements, ctx),
            Self::Pgm105 => super::pgm105::check(*self, statements, ctx),
            Self::Pgm106 => super::pgm106::check(*self, statements, ctx),
            Self::Pgm201 => super::pgm201::check(*self, statements, ctx),
            Self::Pgm202 => super::pgm202::check(*self, statements, ctx),
            Self::Pgm203 => super::pgm203::check(*self, statements, ctx),
            Self::Pgm204 => super::pgm204::check(*self, statements, ctx),
            Self::Pgm205 => super::pgm205::check(*self, statements, ctx),
            Self::Pgm301 => super::pgm301::check(*self, statements, ctx),
            Self::Pgm302 => super::pgm302::check(*self, statements, ctx),
            Self::Pgm303 => super::pgm303::check(*self, statements, ctx),
            Self::Pgm401 => super::pgm401::check(*self, statements, ctx),
            Self::Pgm402 => super::pgm402::check(*self, statements, ctx),
            Self::Pgm403 => super::pgm403::check(*self, statements, ctx),
            Self::Pgm501 => super::pgm501::check(*self, statements, ctx),
            Self::Pgm502 => super::pgm502::check(*self, statements, ctx),
            Self::Pgm503 => super::pgm503::check(*self, statements, ctx),
            Self::Pgm504 => super::pgm504::check(*self, statements, ctx),
            Self::Pgm505 => super::pgm505::check(*self, statements, ctx),
            Self::Pgm506 => super::pgm506::check(*self, statements, ctx),
            Self::Pgm901 => vec![],
        }
    }
}
