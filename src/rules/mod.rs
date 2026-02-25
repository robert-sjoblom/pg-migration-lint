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
// Rule ID enums
// ---------------------------------------------------------------------------

/// Strongly-typed rule identifier.
///
/// Wraps the seven rule families so that match statements are exhaustive:
/// adding a new variant forces updates in `sonarqube_meta()`, `effort_minutes()`,
/// and everywhere else a rule ID is dispatched on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RuleId {
    /// Unsafe DDL rules (0xx series).
    UnsafeDdl(UnsafeDdlRule),
    /// Type anti-pattern rules (1xx series).
    TypeAntiPattern(TypeAntiPatternRule),
    /// Destructive operation rules (2xx series).
    Destructive(DestructiveRule),
    /// DML in migrations rules (3xx series).
    Dml(DmlRule),
    /// Idempotency guard rules (4xx series).
    Idempotency(IdempotencyRule),
    /// Schema design & informational rules (5xx series).
    SchemaDesign(SchemaDesignRule),
    /// Meta-behavior rules (9xx series).
    Meta(MetaRule),
}

impl RuleId {
    /// Zero-allocation string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            RuleId::UnsafeDdl(r) => match r {
                UnsafeDdlRule::Pgm001 => "PGM001",
                UnsafeDdlRule::Pgm002 => "PGM002",
                UnsafeDdlRule::Pgm003 => "PGM003",
                UnsafeDdlRule::Pgm004 => "PGM004",
                UnsafeDdlRule::Pgm005 => "PGM005",
                UnsafeDdlRule::Pgm006 => "PGM006",
                UnsafeDdlRule::Pgm007 => "PGM007",
                UnsafeDdlRule::Pgm008 => "PGM008",
                UnsafeDdlRule::Pgm009 => "PGM009",
                UnsafeDdlRule::Pgm010 => "PGM010",
                UnsafeDdlRule::Pgm011 => "PGM011",
                UnsafeDdlRule::Pgm012 => "PGM012",
                UnsafeDdlRule::Pgm013 => "PGM013",
                UnsafeDdlRule::Pgm014 => "PGM014",
                UnsafeDdlRule::Pgm015 => "PGM015",
                UnsafeDdlRule::Pgm016 => "PGM016",
                UnsafeDdlRule::Pgm017 => "PGM017",
                UnsafeDdlRule::Pgm018 => "PGM018",
            },
            RuleId::TypeAntiPattern(r) => match r {
                TypeAntiPatternRule::Pgm101 => "PGM101",
                TypeAntiPatternRule::Pgm102 => "PGM102",
                TypeAntiPatternRule::Pgm103 => "PGM103",
                TypeAntiPatternRule::Pgm104 => "PGM104",
                TypeAntiPatternRule::Pgm105 => "PGM105",
                TypeAntiPatternRule::Pgm106 => "PGM106",
            },
            RuleId::Destructive(r) => match r {
                DestructiveRule::Pgm201 => "PGM201",
                DestructiveRule::Pgm202 => "PGM202",
                DestructiveRule::Pgm203 => "PGM203",
                DestructiveRule::Pgm204 => "PGM204",
            },
            RuleId::Dml(r) => match r {
                DmlRule::Pgm301 => "PGM301",
                DmlRule::Pgm302 => "PGM302",
                DmlRule::Pgm303 => "PGM303",
            },
            RuleId::Idempotency(r) => match r {
                IdempotencyRule::Pgm401 => "PGM401",
                IdempotencyRule::Pgm402 => "PGM402",
                IdempotencyRule::Pgm403 => "PGM403",
            },
            RuleId::SchemaDesign(r) => match r {
                SchemaDesignRule::Pgm501 => "PGM501",
                SchemaDesignRule::Pgm502 => "PGM502",
                SchemaDesignRule::Pgm503 => "PGM503",
                SchemaDesignRule::Pgm504 => "PGM504",
                SchemaDesignRule::Pgm505 => "PGM505",
                SchemaDesignRule::Pgm506 => "PGM506",
            },
            RuleId::Meta(MetaRule::Pgm901) => "PGM901",
        }
    }
}

impl RuleId {
    /// The family prefix string (e.g. "0xx", "1xx", … "9xx").
    pub fn family_prefix(&self) -> &'static str {
        match self {
            RuleId::UnsafeDdl(_) => "0xx",
            RuleId::TypeAntiPattern(_) => "1xx",
            RuleId::Destructive(_) => "2xx",
            RuleId::Dml(_) => "3xx",
            RuleId::Idempotency(_) => "4xx",
            RuleId::SchemaDesign(_) => "5xx",
            RuleId::Meta(_) => "9xx",
        }
    }

    /// Human-readable family name (e.g. "Unsafe DDL", "Type Anti-pattern").
    pub fn family_name(&self) -> &'static str {
        match self {
            RuleId::UnsafeDdl(_) => "Unsafe DDL",
            RuleId::TypeAntiPattern(_) => "Type Anti-pattern",
            RuleId::Destructive(_) => "Destructive Operation",
            RuleId::Dml(_) => "DML in Migration",
            RuleId::Idempotency(_) => "Idempotency Guard",
            RuleId::SchemaDesign(_) => "Schema Design",
            RuleId::Meta(_) => "Meta-behavior",
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
        match s {
            "PGM001" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001)),
            "PGM002" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm002)),
            "PGM003" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm003)),
            "PGM004" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm004)),
            "PGM005" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm005)),
            "PGM006" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm006)),
            "PGM007" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm007)),
            "PGM008" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm008)),
            "PGM009" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm009)),
            "PGM010" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm010)),
            "PGM011" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm011)),
            "PGM012" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm012)),
            "PGM013" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm013)),
            "PGM014" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm014)),
            "PGM015" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm015)),
            "PGM016" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm016)),
            "PGM017" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm017)),
            "PGM018" => Ok(RuleId::UnsafeDdl(UnsafeDdlRule::Pgm018)),
            "PGM101" => Ok(RuleId::TypeAntiPattern(TypeAntiPatternRule::Pgm101)),
            "PGM102" => Ok(RuleId::TypeAntiPattern(TypeAntiPatternRule::Pgm102)),
            "PGM103" => Ok(RuleId::TypeAntiPattern(TypeAntiPatternRule::Pgm103)),
            "PGM104" => Ok(RuleId::TypeAntiPattern(TypeAntiPatternRule::Pgm104)),
            "PGM105" => Ok(RuleId::TypeAntiPattern(TypeAntiPatternRule::Pgm105)),
            "PGM106" => Ok(RuleId::TypeAntiPattern(TypeAntiPatternRule::Pgm106)),
            "PGM201" => Ok(RuleId::Destructive(DestructiveRule::Pgm201)),
            "PGM202" => Ok(RuleId::Destructive(DestructiveRule::Pgm202)),
            "PGM203" => Ok(RuleId::Destructive(DestructiveRule::Pgm203)),
            "PGM204" => Ok(RuleId::Destructive(DestructiveRule::Pgm204)),
            "PGM301" => Ok(RuleId::Dml(DmlRule::Pgm301)),
            "PGM302" => Ok(RuleId::Dml(DmlRule::Pgm302)),
            "PGM303" => Ok(RuleId::Dml(DmlRule::Pgm303)),
            "PGM401" => Ok(RuleId::Idempotency(IdempotencyRule::Pgm401)),
            "PGM402" => Ok(RuleId::Idempotency(IdempotencyRule::Pgm402)),
            "PGM403" => Ok(RuleId::Idempotency(IdempotencyRule::Pgm403)),
            "PGM501" => Ok(RuleId::SchemaDesign(SchemaDesignRule::Pgm501)),
            "PGM502" => Ok(RuleId::SchemaDesign(SchemaDesignRule::Pgm502)),
            "PGM503" => Ok(RuleId::SchemaDesign(SchemaDesignRule::Pgm503)),
            "PGM504" => Ok(RuleId::SchemaDesign(SchemaDesignRule::Pgm504)),
            "PGM505" => Ok(RuleId::SchemaDesign(SchemaDesignRule::Pgm505)),
            "PGM506" => Ok(RuleId::SchemaDesign(SchemaDesignRule::Pgm506)),
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
            Self::UnsafeDdl(rule) => rule.into(),
            Self::TypeAntiPattern(rule) => rule.into(),
            Self::Destructive(rule) => rule.into(),
            Self::Dml(rule) => rule.into(),
            Self::Idempotency(rule) => rule.into(),
            Self::SchemaDesign(rule) => rule.into(),
            Self::Meta(rule) => rule.into(),
        }
    }

    fn description(&self) -> &'static str {
        match *self {
            Self::UnsafeDdl(rule) => rule.description(),
            Self::TypeAntiPattern(rule) => rule.description(),
            Self::Destructive(rule) => rule.description(),
            Self::Dml(rule) => rule.description(),
            Self::Idempotency(rule) => rule.description(),
            Self::SchemaDesign(rule) => rule.description(),
            Self::Meta(rule) => rule.description(),
        }
    }

    fn explain(&self) -> &'static str {
        match *self {
            Self::UnsafeDdl(rule) => rule.explain(),
            Self::TypeAntiPattern(rule) => rule.explain(),
            Self::Destructive(rule) => rule.explain(),
            Self::Dml(rule) => rule.explain(),
            Self::Idempotency(rule) => rule.explain(),
            Self::SchemaDesign(rule) => rule.explain(),
            Self::Meta(rule) => rule.explain(),
        }
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        match *self {
            Self::UnsafeDdl(rule) => rule.check(*self, statements, ctx),
            Self::TypeAntiPattern(rule) => rule.check(*self, statements, ctx),
            Self::Destructive(rule) => rule.check(*self, statements, ctx),
            Self::Dml(rule) => rule.check(*self, statements, ctx),
            Self::Idempotency(rule) => rule.check(*self, statements, ctx),
            Self::SchemaDesign(rule) => rule.check(*self, statements, ctx),
            Self::Meta(rule) => rule.check(*self, statements, ctx),
        }
    }
}

/// Unsafe DDL rules (0xx series).
///
/// These detect locking, rewrite, and schema-integrity issues in DDL migrations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, EnumIter)]
pub enum UnsafeDdlRule {
    /// `CREATE INDEX` without `CONCURRENTLY` on existing tables.
    Pgm001,
    /// `DROP INDEX` without `CONCURRENTLY` on existing tables.
    Pgm002,
    /// Concurrent index operations inside a transaction.
    Pgm003,
    /// `DETACH PARTITION` without `CONCURRENTLY` on existing tables.
    Pgm004,
    /// `ATTACH PARTITION` of existing table without pre-validated `CHECK`.
    Pgm005,
    /// Volatile function defaults on columns.
    Pgm006,
    /// Column type changes on existing tables.
    Pgm007,
    /// Adding a `NOT NULL` column without a `DEFAULT` to an existing table.
    Pgm008,
    /// Dropping a column from an existing table.
    Pgm009,
    /// Dropping a column that participates in a unique constraint or unique index.
    Pgm010,
    /// Dropping a column that participates in the table's primary key.
    Pgm011,
    /// Dropping a column that participates in a foreign key constraint.
    Pgm012,
    /// `SET NOT NULL` on an existing table column.
    Pgm013,
    /// Adding a `FOREIGN KEY` without `NOT VALID` on an existing table.
    Pgm014,
    /// Adding a `CHECK` constraint without `NOT VALID` on an existing table.
    Pgm015,
    /// Adding a `PRIMARY KEY` without a prior unique index.
    Pgm016,
    /// Adding a `UNIQUE` constraint without a pre-existing unique index.
    Pgm017,
    /// `CLUSTER` on an existing table (ACCESS EXCLUSIVE lock for full rewrite).
    Pgm018,
}

impl UnsafeDdlRule {
    fn description(&self) -> &'static str {
        match *self {
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
            Self::Pgm008 => pgm008::check(rule, statements, ctx),
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
        }
    }
}

impl From<UnsafeDdlRule> for Severity {
    fn from(value: UnsafeDdlRule) -> Self {
        match value {
            UnsafeDdlRule::Pgm001
            | UnsafeDdlRule::Pgm002
            | UnsafeDdlRule::Pgm003
            | UnsafeDdlRule::Pgm004
            | UnsafeDdlRule::Pgm007
            | UnsafeDdlRule::Pgm008
            | UnsafeDdlRule::Pgm013
            | UnsafeDdlRule::Pgm014
            | UnsafeDdlRule::Pgm015
            | UnsafeDdlRule::Pgm017
            | UnsafeDdlRule::Pgm018 => Self::Critical,
            UnsafeDdlRule::Pgm005 | UnsafeDdlRule::Pgm011 | UnsafeDdlRule::Pgm016 => Self::Major,
            UnsafeDdlRule::Pgm006 | UnsafeDdlRule::Pgm010 | UnsafeDdlRule::Pgm012 => Self::Minor,
            UnsafeDdlRule::Pgm009 => Self::Info,
        }
    }
}

/// "Don't Do This" type anti-pattern rules (PGM101–PGM106).
///
/// These flag column types that should be avoided (e.g. `money`, `serial`, `char(n)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, EnumIter)]
pub enum TypeAntiPatternRule {
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
    Pgm106,
}

impl TypeAntiPatternRule {
    fn description(&self) -> &'static str {
        match *self {
            Self::Pgm101 => pgm101::DESCRIPTION,
            Self::Pgm102 => pgm102::DESCRIPTION,
            Self::Pgm103 => pgm103::DESCRIPTION,
            Self::Pgm104 => pgm104::DESCRIPTION,
            Self::Pgm105 => pgm105::DESCRIPTION,
            Self::Pgm106 => pgm106::DESCRIPTION,
        }
    }

    fn explain(&self) -> &'static str {
        match *self {
            Self::Pgm101 => pgm101::EXPLAIN,
            Self::Pgm102 => pgm102::EXPLAIN,
            Self::Pgm103 => pgm103::EXPLAIN,
            Self::Pgm104 => pgm104::EXPLAIN,
            Self::Pgm105 => pgm105::EXPLAIN,
            Self::Pgm106 => pgm106::EXPLAIN,
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
            Self::Pgm106 => pgm106::check(rule, statements, ctx),
        }
    }
}

impl From<TypeAntiPatternRule> for Severity {
    fn from(value: TypeAntiPatternRule) -> Self {
        match value {
            TypeAntiPatternRule::Pgm101
            | TypeAntiPatternRule::Pgm102
            | TypeAntiPatternRule::Pgm103
            | TypeAntiPatternRule::Pgm104
            | TypeAntiPatternRule::Pgm106 => Self::Minor,
            TypeAntiPatternRule::Pgm105 => Self::Info,
        }
    }
}

/// Destructive operation rules (PGM2xx).
///
/// These flag operations that can cause data loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, EnumIter)]
pub enum DestructiveRule {
    /// Dropping an existing table.
    Pgm201,
    /// `DROP TABLE CASCADE` on existing table.
    Pgm202,
    /// `TRUNCATE TABLE` on existing table.
    Pgm203,
    /// `TRUNCATE TABLE CASCADE` on existing table.
    Pgm204,
}

impl DestructiveRule {
    fn description(&self) -> &'static str {
        match *self {
            Self::Pgm201 => pgm201::DESCRIPTION,
            Self::Pgm202 => pgm202::DESCRIPTION,
            Self::Pgm203 => pgm203::DESCRIPTION,
            Self::Pgm204 => pgm204::DESCRIPTION,
        }
    }

    fn explain(&self) -> &'static str {
        match *self {
            Self::Pgm201 => pgm201::EXPLAIN,
            Self::Pgm202 => pgm202::EXPLAIN,
            Self::Pgm203 => pgm203::EXPLAIN,
            Self::Pgm204 => pgm204::EXPLAIN,
        }
    }

    fn check(
        &self,
        rule: impl Rule,
        statements: &[Located<IrNode>],
        ctx: &LintContext<'_>,
    ) -> Vec<Finding> {
        match *self {
            Self::Pgm201 => pgm201::check(rule, statements, ctx),
            Self::Pgm202 => pgm202::check(rule, statements, ctx),
            Self::Pgm203 => pgm203::check(rule, statements, ctx),
            Self::Pgm204 => pgm204::check(rule, statements, ctx),
        }
    }
}

impl From<DestructiveRule> for Severity {
    fn from(value: DestructiveRule) -> Self {
        match value {
            DestructiveRule::Pgm201 | DestructiveRule::Pgm203 => Self::Minor,
            DestructiveRule::Pgm202 | DestructiveRule::Pgm204 => Self::Major,
        }
    }
}

/// DML in migrations rules (PGM3xx).
///
/// These flag DML statements (INSERT, UPDATE, DELETE) in migration files
/// targeting pre-existing tables, which may indicate unintentional data
/// modification or performance risks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, EnumIter)]
pub enum DmlRule {
    /// `INSERT INTO` existing table in migration.
    Pgm301,
    /// `UPDATE` on existing table in migration.
    Pgm302,
    /// `DELETE FROM` existing table in migration.
    Pgm303,
}

impl DmlRule {
    fn description(&self) -> &'static str {
        match *self {
            Self::Pgm301 => pgm301::DESCRIPTION,
            Self::Pgm302 => pgm302::DESCRIPTION,
            Self::Pgm303 => pgm303::DESCRIPTION,
        }
    }

    fn explain(&self) -> &'static str {
        match *self {
            Self::Pgm301 => pgm301::EXPLAIN,
            Self::Pgm302 => pgm302::EXPLAIN,
            Self::Pgm303 => pgm303::EXPLAIN,
        }
    }

    fn check(
        &self,
        rule: impl Rule,
        statements: &[Located<IrNode>],
        ctx: &LintContext<'_>,
    ) -> Vec<Finding> {
        match *self {
            Self::Pgm301 => pgm301::check(rule, statements, ctx),
            Self::Pgm302 => pgm302::check(rule, statements, ctx),
            Self::Pgm303 => pgm303::check(rule, statements, ctx),
        }
    }
}

impl From<DmlRule> for Severity {
    fn from(value: DmlRule) -> Self {
        match value {
            DmlRule::Pgm301 => Self::Info,
            DmlRule::Pgm302 | DmlRule::Pgm303 => Self::Minor,
        }
    }
}

/// Idempotency guard rules (PGM4xx).
///
/// These flag missing safety guards like `IF EXISTS` / `IF NOT EXISTS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, EnumIter)]
pub enum IdempotencyRule {
    /// Missing `IF EXISTS` on `DROP TABLE` / `DROP INDEX`.
    Pgm401,
    /// Missing `IF NOT EXISTS` on `CREATE TABLE` / `CREATE INDEX`.
    Pgm402,
    /// `CREATE TABLE IF NOT EXISTS` for already-existing table (misleading no-op).
    Pgm403,
}

impl IdempotencyRule {
    fn description(&self) -> &'static str {
        match *self {
            Self::Pgm401 => pgm401::DESCRIPTION,
            Self::Pgm402 => pgm402::DESCRIPTION,
            Self::Pgm403 => pgm403::DESCRIPTION,
        }
    }

    fn explain(&self) -> &'static str {
        match *self {
            Self::Pgm401 => pgm401::EXPLAIN,
            Self::Pgm402 => pgm402::EXPLAIN,
            Self::Pgm403 => pgm403::EXPLAIN,
        }
    }

    fn check(
        &self,
        rule: impl Rule,
        statements: &[Located<IrNode>],
        ctx: &LintContext<'_>,
    ) -> Vec<Finding> {
        match *self {
            Self::Pgm401 => pgm401::check(rule, statements, ctx),
            Self::Pgm402 => pgm402::check(rule, statements, ctx),
            Self::Pgm403 => pgm403::check(rule, statements, ctx),
        }
    }
}

impl From<IdempotencyRule> for Severity {
    fn from(value: IdempotencyRule) -> Self {
        match value {
            IdempotencyRule::Pgm401 | IdempotencyRule::Pgm402 | IdempotencyRule::Pgm403 => {
                Self::Minor
            }
        }
    }
}

/// Schema design & informational rules (PGM5xx).
///
/// These flag schema quality issues and informational findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, EnumIter)]
pub enum SchemaDesignRule {
    /// Foreign keys without a covering index on the referencing table.
    Pgm501,
    /// Tables created without a primary key.
    Pgm502,
    /// Tables using `UNIQUE NOT NULL` instead of a proper `PRIMARY KEY`.
    Pgm503,
    /// `ALTER TABLE ... RENAME TO` on existing tables.
    Pgm504,
    /// `RENAME COLUMN` on an existing table.
    Pgm505,
    /// `CREATE UNLOGGED TABLE`.
    Pgm506,
}

impl SchemaDesignRule {
    fn description(&self) -> &'static str {
        match *self {
            Self::Pgm501 => pgm501::DESCRIPTION,
            Self::Pgm502 => pgm502::DESCRIPTION,
            Self::Pgm503 => pgm503::DESCRIPTION,
            Self::Pgm504 => pgm504::DESCRIPTION,
            Self::Pgm505 => pgm505::DESCRIPTION,
            Self::Pgm506 => pgm506::DESCRIPTION,
        }
    }

    fn explain(&self) -> &'static str {
        match *self {
            Self::Pgm501 => pgm501::EXPLAIN,
            Self::Pgm502 => pgm502::EXPLAIN,
            Self::Pgm503 => pgm503::EXPLAIN,
            Self::Pgm504 => pgm504::EXPLAIN,
            Self::Pgm505 => pgm505::EXPLAIN,
            Self::Pgm506 => pgm506::EXPLAIN,
        }
    }

    fn check(
        &self,
        rule: impl Rule,
        statements: &[Located<IrNode>],
        ctx: &LintContext<'_>,
    ) -> Vec<Finding> {
        match *self {
            Self::Pgm501 => pgm501::check(rule, statements, ctx),
            Self::Pgm502 => pgm502::check(rule, statements, ctx),
            Self::Pgm503 => pgm503::check(rule, statements, ctx),
            Self::Pgm504 => pgm504::check(rule, statements, ctx),
            Self::Pgm505 => pgm505::check(rule, statements, ctx),
            Self::Pgm506 => pgm506::check(rule, statements, ctx),
        }
    }
}

impl From<SchemaDesignRule> for Severity {
    fn from(value: SchemaDesignRule) -> Self {
        match value {
            SchemaDesignRule::Pgm501 | SchemaDesignRule::Pgm502 => Self::Major,
            SchemaDesignRule::Pgm503
            | SchemaDesignRule::Pgm504
            | SchemaDesignRule::Pgm505
            | SchemaDesignRule::Pgm506 => Self::Info,
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
        UnsafeDdlRule::iter()
            .map(RuleId::UnsafeDdl)
            .chain(TypeAntiPatternRule::iter().map(RuleId::TypeAntiPattern))
            .chain(DestructiveRule::iter().map(RuleId::Destructive))
            .chain(DmlRule::iter().map(RuleId::Dml))
            .chain(IdempotencyRule::iter().map(RuleId::Idempotency))
            .chain(SchemaDesignRule::iter().map(RuleId::SchemaDesign))
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
                rule_id: RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001),
                severity: Severity::Critical,
                message: "test".to_string(),
                file: PathBuf::from("test.sql"),
                start_line: 1,
                end_line: 1,
            },
            Finding {
                rule_id: RuleId::SchemaDesign(SchemaDesignRule::Pgm502),
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
        }
    }

    #[test]
    fn test_explain_output_snapshots() {
        let mut registry = RuleRegistry::new();
        registry.register_defaults();

        for rule in registry.iter() {
            let output = format!(
                "Rule: {}\nSeverity: {}\nDescription: {}\n\n{}",
                rule.id(),
                rule.default_severity(),
                rule.description(),
                rule.explain()
            );
            insta::assert_snapshot!(format!("explain_{}", rule.id()), output);
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
        let all: Vec<RuleId> = UnsafeDdlRule::iter()
            .map(RuleId::UnsafeDdl)
            .chain(TypeAntiPatternRule::iter().map(RuleId::TypeAntiPattern))
            .chain(DestructiveRule::iter().map(RuleId::Destructive))
            .chain(DmlRule::iter().map(RuleId::Dml))
            .chain(IdempotencyRule::iter().map(RuleId::Idempotency))
            .chain(SchemaDesignRule::iter().map(RuleId::SchemaDesign))
            .chain(MetaRule::iter().map(RuleId::Meta))
            .collect();
        for id in &all {
            let s = id.to_string();
            let parsed: RuleId = s.parse().unwrap_or_else(|_| panic!("failed to parse {s}"));
            assert_eq!(*id, parsed, "round-trip failed for {s}");
            assert_eq!(id.as_str(), s.as_str());
        }
        let expected = UnsafeDdlRule::iter().count()
            + TypeAntiPatternRule::iter().count()
            + DestructiveRule::iter().count()
            + DmlRule::iter().count()
            + IdempotencyRule::iter().count()
            + SchemaDesignRule::iter().count()
            + MetaRule::iter().count();
        assert_eq!(all.len(), expected);
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
        // UnsafeDdl < TypeAntiPattern < Destructive < Idempotency < SchemaDesign < Meta
        assert!(
            RuleId::UnsafeDdl(UnsafeDdlRule::Pgm017)
                < RuleId::TypeAntiPattern(TypeAntiPatternRule::Pgm101)
        );
        assert!(
            RuleId::TypeAntiPattern(TypeAntiPatternRule::Pgm106)
                < RuleId::Destructive(DestructiveRule::Pgm201)
        );
        assert!(RuleId::Destructive(DestructiveRule::Pgm201) < RuleId::Dml(DmlRule::Pgm301));
        assert!(RuleId::Dml(DmlRule::Pgm303) < RuleId::Idempotency(IdempotencyRule::Pgm401));
        assert!(
            RuleId::Idempotency(IdempotencyRule::Pgm402)
                < RuleId::SchemaDesign(SchemaDesignRule::Pgm501)
        );
        assert!(RuleId::SchemaDesign(SchemaDesignRule::Pgm506) < RuleId::Meta(MetaRule::Pgm901));
        // Within UnsafeDdl family
        assert!(
            RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001) < RuleId::UnsafeDdl(UnsafeDdlRule::Pgm017)
        );
    }

    #[test]
    fn test_rule_id_serialize_json() {
        let id = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm003);
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, "\"PGM003\"");
    }

    #[test]
    fn test_parse_rule_id_error_display() {
        let err = "BOGUS".parse::<RuleId>().unwrap_err();
        assert_eq!(err.to_string(), "unknown rule ID: 'BOGUS'");
    }

    #[test]
    fn meta_rule_pgm901_description_is_non_empty() {
        let rule_id = RuleId::Meta(MetaRule::Pgm901);
        let desc = rule_id.description();
        assert!(!desc.is_empty(), "PGM901 description should not be empty");
        assert!(
            desc.contains("Meta"),
            "PGM901 description should mention Meta"
        );
    }

    #[test]
    fn meta_rule_pgm901_explain_is_non_empty() {
        let rule_id = RuleId::Meta(MetaRule::Pgm901);
        let explain = rule_id.explain();
        assert!(!explain.is_empty(), "PGM901 explain should not be empty");
        assert!(
            explain.contains("INFO"),
            "PGM901 explain should mention INFO"
        );
    }
}
