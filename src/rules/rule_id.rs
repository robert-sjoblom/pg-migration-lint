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
    /// `VACUUM FULL` on an existing table (ACCESS EXCLUSIVE lock for full rewrite).
    #[strum(serialize = "PGM021")]
    Pgm021,
    /// `REINDEX` without `CONCURRENTLY` (ACCESS EXCLUSIVE lock).
    #[strum(serialize = "PGM022")]
    Pgm022,

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
    /// Primary key column uses `integer` or `smallint` instead of `bigint`.
    #[strum(serialize = "PGM107")]
    Pgm107,
    /// `varchar(n)` column (prefer `text`).
    #[strum(serialize = "PGM108")]
    Pgm108,
    /// Floating-point column type (`real`/`double precision`).
    #[strum(serialize = "PGM109")]
    Pgm109,

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
    /// Duplicate or redundant index (prefix of another index on the same table).
    #[strum(serialize = "PGM508")]
    Pgm508,
    /// Mixed-case identifier or reserved word requires double-quoting.
    #[strum(serialize = "PGM509")]
    Pgm509,

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

/// Generate the `impl Rule for RuleId` by dispatching each variant to
/// its module's `DEFAULT_SEVERITY`, `DESCRIPTION`, `EXPLAIN`, and `check`.
///
/// PGM901 is a meta-rule with no module — it's handled inline.
macro_rules! dispatch_rules {
    ( $( $variant:ident => $module:ident ),+ $(,)? ) => {
        impl Rule for RuleId {
            fn id(&self) -> Self {
                *self
            }

            fn default_severity(&self) -> Severity {
                match self {
                    $( Self::$variant => super::$module::DEFAULT_SEVERITY, )+
                    Self::Pgm901 => Severity::Info,
                }
            }

            fn description(&self) -> &'static str {
                match self {
                    $( Self::$variant => super::$module::DESCRIPTION, )+
                    Self::Pgm901 => {
                        "Meta rules alter the behavior of other rules, they are not rules themselves"
                    }
                }
            }

            fn explain(&self) -> &'static str {
                match self {
                    $( Self::$variant => super::$module::EXPLAIN, )+
                    Self::Pgm901 => "This rule caps severity of triggered rules to INFO (not in SonarQube)",
                }
            }

            fn check(
                &self,
                statements: &[Located<IrNode>],
                ctx: &LintContext<'_>,
            ) -> Vec<Finding> {
                match self {
                    $( Self::$variant => super::$module::check(*self, statements, ctx), )+
                    Self::Pgm901 => vec![],
                }
            }
        }
    };
}

dispatch_rules! {
    // 0xx — Unsafe DDL
    Pgm001 => pgm001,
    Pgm002 => pgm002,
    Pgm003 => pgm003,
    Pgm004 => pgm004,
    Pgm005 => pgm005,
    Pgm006 => pgm006,
    Pgm007 => pgm007,
    Pgm008 => pgm008,
    Pgm009 => pgm009,
    Pgm010 => pgm010,
    Pgm011 => pgm011,
    Pgm012 => pgm012,
    Pgm013 => pgm013,
    Pgm014 => pgm014,
    Pgm015 => pgm015,
    Pgm016 => pgm016,
    Pgm017 => pgm017,
    Pgm018 => pgm018,
    Pgm019 => pgm019,
    Pgm020 => pgm020,
    Pgm021 => pgm021,
    Pgm022 => pgm022,
    // 1xx — Type anti-patterns
    Pgm101 => pgm101,
    Pgm102 => pgm102,
    Pgm103 => pgm103,
    Pgm104 => pgm104,
    Pgm105 => pgm105,
    Pgm106 => pgm106,
    Pgm107 => pgm107,
    Pgm108 => pgm108,
    Pgm109 => pgm109,
    // 2xx — Destructive operations
    Pgm201 => pgm201,
    Pgm202 => pgm202,
    Pgm203 => pgm203,
    Pgm204 => pgm204,
    Pgm205 => pgm205,
    // 3xx — DML in migrations
    Pgm301 => pgm301,
    Pgm302 => pgm302,
    Pgm303 => pgm303,
    // 4xx — Idempotency guards
    Pgm401 => pgm401,
    Pgm402 => pgm402,
    Pgm403 => pgm403,
    // 5xx — Schema design
    Pgm501 => pgm501,
    Pgm502 => pgm502,
    Pgm503 => pgm503,
    Pgm504 => pgm504,
    Pgm505 => pgm505,
    Pgm506 => pgm506,
    Pgm507 => pgm507,
    Pgm508 => pgm508,
    Pgm509 => pgm509,
}
