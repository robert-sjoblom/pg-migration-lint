//! PGM009 — `ALTER COLUMN TYPE` on existing table
//!
//! Detects `ALTER TABLE ... ALTER COLUMN ... TYPE ...` on tables that already
//! exist in the catalog. Most type changes require a full table rewrite under
//! an `ACCESS EXCLUSIVE` lock. A hardcoded allowlist of safe (binary-coercible)
//! casts suppresses the finding for known safe conversions.

use crate::parser::ir::{AlterTableAction, IrNode, Located, TypeName};
use crate::rules::{Finding, LintContext, Rule, Severity, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str = "ALTER COLUMN TYPE on existing table causes table rewrite";

pub(super) const EXPLAIN: &str = "PGM009 — ALTER COLUMN TYPE on existing table\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... ALTER COLUMN ... TYPE ... on a table that already\n\
         exists in the database (not created in the same set of changed files).\n\
         \n\
         Why it's dangerous:\n\
         Most type changes require a full table rewrite and an ACCESS EXCLUSIVE\n\
         lock for the duration. For large tables, this causes extended downtime.\n\
         Binary-coercible casts (e.g., varchar widening) do NOT rewrite.\n\
         \n\
         Safe casts (no finding):\n\
         - varchar(N) -> varchar(M) where M > N\n\
         - varchar(N) -> text\n\
         - numeric(P,S) -> numeric(P2,S) where P2 > P and same scale\n\
         - bit(N) -> bit(M) where M > N\n\
         - varbit(N) -> varbit(M) where M > N\n\
         \n\
         INFO cast:\n\
         - timestamp -> timestamptz (safe in PG 15+ with UTC timezone;\n\
           verify your timezone config)\n\
         \n\
         All other type changes fire as CRITICAL.\n\
         \n\
         Example (bad):\n\
           ALTER TABLE orders ALTER COLUMN amount TYPE bigint;\n\
         \n\
         Fix:\n\
           -- Create a new column, backfill, and swap:\n\
           ALTER TABLE orders ADD COLUMN amount_new bigint;\n\
           UPDATE orders SET amount_new = amount;\n\
           ALTER TABLE orders DROP COLUMN amount;\n\
           ALTER TABLE orders RENAME COLUMN amount_new TO amount;";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    alter_table_check::check_alter_actions(
        statements,
        ctx,
        TableScope::ExcludeCreatedInChange,
        |at, action, stmt, ctx| {
            let AlterTableAction::AlterColumnType {
                column_name,
                new_type,
                old_type,
            } = action
            else {
                return vec![];
            };

            let table_key = at.name.catalog_key();

            // Resolve old_type: prefer the one from the IR, fall back to catalog.
            let resolved_old_type = old_type.as_ref().or_else(|| {
                ctx.catalog_before
                    .get_table(table_key)
                    .and_then(|t| t.get_column(column_name))
                    .map(|c| &c.type_name)
            });

            let safety = match resolved_old_type {
                Some(old) => is_safe_cast(old, new_type),
                None => {
                    // Cannot determine old type — assume unsafe.
                    CastSafety::Unsafe
                }
            };

            let severity = match safety {
                CastSafety::Safe => return vec![],
                CastSafety::Info => Severity::Info,
                CastSafety::Unsafe => rule.default_severity(),
            };

            let old_display = resolved_old_type
                .map(|t| t.to_string())
                .unwrap_or_else(|| "unknown".to_string());

            vec![Finding::new(
                rule.id(),
                severity,
                format!(
                    "Changing column type on existing table '{table}' \
                     ('{col}': {old} \u{2192} {new}) rewrites the entire table \
                     under an ACCESS EXCLUSIVE lock. For large tables, this causes \
                     extended downtime. Consider creating a new column, backfilling, \
                     and swapping instead.",
                    table = at.name.display_name(),
                    col = column_name,
                    old = old_display,
                    new = new_type,
                ),
                ctx.file,
                &stmt.span,
            )]
        },
    )
}

/// Result of checking whether a type cast is safe.
#[derive(Debug, PartialEq, Eq)]
pub enum CastSafety {
    /// No table rewrite needed — produce no finding.
    Safe,
    /// Conditionally safe — produce an INFO finding.
    Info,
    /// Requires table rewrite — produce a CRITICAL finding.
    Unsafe,
}

/// Determine whether changing from `old` to `new` is a safe (binary-coercible)
/// cast, an INFO-level conditional cast, or an unsafe rewrite.
///
/// Safe casts (no finding):
/// - `varchar(N)` -> `varchar(M)` where M > N
/// - `varchar(N)` -> `text` (text has no modifiers)
/// - `numeric(P,S)` -> `numeric(P2,S)` where P2 > P and same scale
/// - `bit(N)` -> `bit(M)` where M > N
/// - `varbit(N)` -> `varbit(M)` where M > N
///
/// INFO casts:
/// - `timestamp` -> `timestamptz`
///
/// Everything else: Unsafe.
pub fn is_safe_cast(old: &TypeName, new: &TypeName) -> CastSafety {
    let old_name = old.name.to_lowercase();
    let new_name = new.name.to_lowercase();

    // varchar/character varying widening or varchar -> text
    if is_varchar_type(&old_name) {
        if is_varchar_type(&new_name) {
            return check_widening_single_modifier(old, new);
        }
        if new_name == "text" {
            // varchar(N) -> text is always safe (removes length limit)
            return CastSafety::Safe;
        }
    }

    // numeric/decimal precision widening (same scale)
    if is_numeric_type(&old_name) && is_numeric_type(&new_name) {
        return check_numeric_widening(old, new);
    }

    // bit widening
    if old_name == "bit" && new_name == "bit" {
        return check_widening_single_modifier(old, new);
    }

    // varbit widening
    if is_varbit_type(&old_name) && is_varbit_type(&new_name) {
        return check_widening_single_modifier(old, new);
    }

    // timestamp -> timestamptz
    if is_timestamp_type(&old_name) && is_timestamptz_type(&new_name) {
        return CastSafety::Info;
    }

    CastSafety::Unsafe
}

/// Check if a type name is a varchar variant.
fn is_varchar_type(name: &str) -> bool {
    matches!(name, "varchar" | "character varying")
}

/// Check if a type name is a numeric variant.
fn is_numeric_type(name: &str) -> bool {
    matches!(name, "numeric" | "decimal")
}

/// Check if a type name is a varbit variant.
fn is_varbit_type(name: &str) -> bool {
    matches!(name, "varbit" | "bit varying")
}

/// Check if a type name is timestamp (without timezone).
fn is_timestamp_type(name: &str) -> bool {
    matches!(name, "timestamp" | "timestamp without time zone")
}

/// Check if a type name is timestamptz (with timezone).
fn is_timestamptz_type(name: &str) -> bool {
    matches!(name, "timestamptz" | "timestamp with time zone")
}

/// For types with a single modifier (e.g., varchar(N), bit(N)):
/// check if new modifier >= old modifier.
fn check_widening_single_modifier(old: &TypeName, new: &TypeName) -> CastSafety {
    match (old.modifiers.first(), new.modifiers.first()) {
        (Some(&old_m), Some(&new_m)) => {
            if new_m >= old_m {
                CastSafety::Safe
            } else {
                CastSafety::Unsafe
            }
        }
        // Old has modifier, new does not (unbounded) — safe (widening)
        (Some(_), None) => CastSafety::Safe,
        // Old has no modifier, new has one — could be narrowing, unsafe
        (None, Some(_)) => CastSafety::Unsafe,
        // Neither has modifiers — same type effectively
        (None, None) => CastSafety::Safe,
    }
}

/// Normalize numeric modifiers: `numeric(P)` is equivalent to `numeric(P, 0)`.
fn normalize_numeric_modifiers(mods: &[i64]) -> (i64, i64) {
    match mods {
        [p, s] => (*p, *s),
        [p] => (*p, 0),
        _ => (-1, -1), // sentinel for unmodified or unexpected
    }
}

/// Check numeric(P,S) -> numeric(P2,S) widening.
/// Safe if: P2 >= P and scale is the same.
fn check_numeric_widening(old: &TypeName, new: &TypeName) -> CastSafety {
    match (old.modifiers.as_slice(), new.modifiers.as_slice()) {
        // Both unmodified (bare `numeric`) — no-op
        ([], []) => CastSafety::Safe,
        // Constrained -> unconstrained is widening (safe)
        (_, []) => CastSafety::Safe,
        // Unconstrained -> constrained is potentially narrowing
        ([], _) => CastSafety::Unsafe,
        // Both have modifiers — normalize and compare
        (old_mods, new_mods) => {
            let (old_p, old_s) = normalize_numeric_modifiers(old_mods);
            let (new_p, new_s) = normalize_numeric_modifiers(new_mods);
            if new_p >= old_p && new_s == old_s {
                CastSafety::Safe
            } else {
                CastSafety::Unsafe
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::test_helpers::{located, make_ctx};
    use crate::rules::{MigrationRule, RuleId};
    use std::collections::HashSet;
    use std::path::PathBuf;

    // --- Unit tests for is_safe_cast ---

    #[test]
    fn test_varchar_widening_safe() {
        let old = TypeName::with_modifiers("varchar", vec![50]);
        let new = TypeName::with_modifiers("varchar", vec![100]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Safe);
    }

    #[test]
    fn test_varchar_to_text_safe() {
        let old = TypeName::with_modifiers("varchar", vec![50]);
        let new = TypeName::simple("text");
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Safe);
    }

    #[test]
    fn test_varchar_narrowing_unsafe() {
        let old = TypeName::with_modifiers("varchar", vec![100]);
        let new = TypeName::with_modifiers("varchar", vec![50]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    #[test]
    fn test_text_to_varchar_unsafe() {
        let old = TypeName::simple("text");
        let new = TypeName::with_modifiers("varchar", vec![100]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    #[test]
    fn test_numeric_widening_same_scale_safe() {
        let old = TypeName::with_modifiers("numeric", vec![10, 2]);
        let new = TypeName::with_modifiers("numeric", vec![12, 2]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Safe);
    }

    #[test]
    fn test_numeric_precision_only_to_precision_scale_safe() {
        // numeric(10) is equivalent to numeric(10, 0) in PostgreSQL
        let old = TypeName::with_modifiers("numeric", vec![10]);
        let new = TypeName::with_modifiers("numeric", vec![12, 0]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Safe);
    }

    #[test]
    fn test_numeric_precision_scale_to_precision_only_safe() {
        let old = TypeName::with_modifiers("numeric", vec![10, 0]);
        let new = TypeName::with_modifiers("numeric", vec![12]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Safe);
    }

    #[test]
    fn test_numeric_identity_after_normalization() {
        // numeric(10) = numeric(10, 0), so this is a no-op
        let old = TypeName::with_modifiers("numeric", vec![10]);
        let new = TypeName::with_modifiers("numeric", vec![10, 0]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Safe);
    }

    #[test]
    fn test_numeric_narrowing_single_modifier_unsafe() {
        // numeric(10) -> numeric(8) is narrowing (both normalize to scale 0)
        let old = TypeName::with_modifiers("numeric", vec![10]);
        let new = TypeName::with_modifiers("numeric", vec![8]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    #[test]
    fn test_numeric_scale_change_via_normalization_unsafe() {
        // numeric(10) = numeric(10,0), so -> numeric(10,2) changes scale
        let old = TypeName::with_modifiers("numeric", vec![10]);
        let new = TypeName::with_modifiers("numeric", vec![10, 2]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    #[test]
    fn test_numeric_bare_to_bare_safe() {
        let old = TypeName::simple("numeric");
        let new = TypeName::simple("numeric");
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Safe);
    }

    #[test]
    fn test_numeric_bare_to_constrained_unsafe() {
        let old = TypeName::simple("numeric");
        let new = TypeName::with_modifiers("numeric", vec![10]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    #[test]
    fn test_numeric_constrained_to_bare_safe() {
        // Removing precision/scale constraints is widening
        let old = TypeName::with_modifiers("numeric", vec![10, 2]);
        let new = TypeName::simple("numeric");
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Safe);
    }

    #[test]
    fn test_numeric_scale_change_unsafe() {
        let old = TypeName::with_modifiers("numeric", vec![10, 2]);
        let new = TypeName::with_modifiers("numeric", vec![10, 4]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    #[test]
    fn test_int_to_bigint_unsafe() {
        let old = TypeName::simple("integer");
        let new = TypeName::simple("bigint");
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    #[test]
    fn test_timestamp_to_timestamptz_info() {
        let old = TypeName::simple("timestamp");
        let new = TypeName::simple("timestamptz");
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Info);
    }

    #[test]
    fn test_totally_different_types_unsafe() {
        let old = TypeName::simple("integer");
        let new = TypeName::simple("text");
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    // --- Component tests for the rule ---

    #[test]
    fn test_varchar_widening_no_finding() {
        let before = CatalogBuilder::new()
            .table("users", |t| {
                t.column("name", "varchar", false).pk(&["name"]);
                // Need to set the column type with modifiers.
                // The builder uses TypeName::simple, so we need direct manipulation.
            })
            .build();
        // Override the column type with modifiers via a custom approach.
        // Since the builder doesn't support modifiers, we build directly.
        let mut before_with_mods = before.clone();
        if let Some(table) = before_with_mods.get_table_mut("users")
            && let Some(col) = table.get_column_mut("name")
        {
            col.type_name = TypeName::with_modifiers("varchar", vec![50]);
        }

        let after = before_with_mods.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before_with_mods, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("users"),
            actions: vec![AlterTableAction::AlterColumnType {
                column_name: "name".to_string(),
                new_type: TypeName::with_modifiers("varchar", vec![100]),
                old_type: None,
            }],
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm009).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_int_to_bigint_fires_critical() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("amount", "integer", false).pk(&["amount"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AlterColumnType {
                column_name: "amount".to_string(),
                new_type: TypeName::simple("bigint"),
                old_type: None,
            }],
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm009).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_timestamp_to_timestamptz_fires_info() {
        let before = CatalogBuilder::new()
            .table("events", |t| {
                t.column("created_at", "timestamp", true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("events"),
            actions: vec![AlterTableAction::AlterColumnType {
                column_name: "created_at".to_string(),
                new_type: TypeName::simple("timestamptz"),
                old_type: None,
            }],
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm009).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("amount", "bigint", false);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AlterColumnType {
                column_name: "amount".to_string(),
                new_type: TypeName::simple("bigint"),
                old_type: None,
            }],
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm009).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    // --- Mutation-killing tests for is_safe_cast helper branches ---

    // Mutant #1: line 55 replace && with || in numeric type check.
    // If `||` were used, numeric->text would enter the numeric widening branch
    // and return Safe (since numeric_constrained_to_bare is Safe).
    // But numeric->text should be Unsafe.
    #[test]
    fn test_numeric_to_text_unsafe() {
        // old is numeric, new is NOT numeric — must be Unsafe.
        // With && -> ||, would incorrectly enter numeric branch.
        let old = TypeName::with_modifiers("numeric", vec![10, 2]);
        let new = TypeName::simple("text");
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    #[test]
    fn test_text_to_numeric_unsafe() {
        // old is NOT numeric, new IS numeric — must be Unsafe.
        // With && -> ||, would incorrectly enter numeric branch.
        let old = TypeName::simple("text");
        let new = TypeName::with_modifiers("numeric", vec![10, 2]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    // Mutants #2, #3, #4: line 60 bit == checks and && vs ||.
    // Need a test proving bit(N)->bit(M) widening IS safe.
    // With == -> != or && -> ||, these would break.
    #[test]
    fn test_bit_widening_safe() {
        // bit(8) -> bit(16) should be Safe (widening).
        // Killed by: == -> != on either operand (would skip the branch),
        // and && -> || (would enter branch when only one side is "bit").
        let old = TypeName::with_modifiers("bit", vec![8]);
        let new = TypeName::with_modifiers("bit", vec![16]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Safe);
    }

    #[test]
    fn test_bit_narrowing_unsafe() {
        // bit(16) -> bit(8) should be Unsafe (narrowing).
        let old = TypeName::with_modifiers("bit", vec![16]);
        let new = TypeName::with_modifiers("bit", vec![8]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    #[test]
    fn test_bit_to_varbit_unsafe() {
        // bit -> varbit is NOT in the allowlist, should be Unsafe.
        // With && -> || on line 60, "bit" on old_name alone would enter
        // the bit branch and return Safe (since widening check with no
        // modifiers returns Safe). This test ensures it stays Unsafe.
        let old = TypeName::with_modifiers("bit", vec![8]);
        let new = TypeName::with_modifiers("varbit", vec![16]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    #[test]
    fn test_varbit_to_bit_unsafe() {
        // varbit -> bit is NOT in the allowlist, should be Unsafe.
        // With && -> || on line 60, "bit" on new_name alone would enter
        // the bit branch.
        let old = TypeName::with_modifiers("varbit", vec![16]);
        let new = TypeName::with_modifiers("bit", vec![8]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    // Mutants #5, #7: line 65 && -> || in varbit check, and line 89 is_varbit_type -> false.
    #[test]
    fn test_varbit_widening_safe() {
        // varbit(8) -> varbit(16) should be Safe.
        // Killed by is_varbit_type returning false (would fall through to Unsafe)
        // and && -> || (would enter branch when only one side is varbit).
        let old = TypeName::with_modifiers("varbit", vec![8]);
        let new = TypeName::with_modifiers("varbit", vec![16]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Safe);
    }

    #[test]
    fn test_bit_varying_widening_safe() {
        // "bit varying" is the alternate name for varbit; tests is_varbit_type.
        let old = TypeName::with_modifiers("bit varying", vec![8]);
        let new = TypeName::with_modifiers("bit varying", vec![16]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Safe);
    }

    #[test]
    fn test_varbit_narrowing_unsafe() {
        let old = TypeName::with_modifiers("varbit", vec![16]);
        let new = TypeName::with_modifiers("varbit", vec![8]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    #[test]
    fn test_varbit_to_text_unsafe() {
        // old is varbit, new is NOT varbit — must be Unsafe.
        // With && -> ||, would incorrectly enter varbit branch.
        let old = TypeName::with_modifiers("varbit", vec![8]);
        let new = TypeName::simple("text");
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    #[test]
    fn test_text_to_varbit_unsafe() {
        // old is NOT varbit, new IS varbit — must be Unsafe.
        // With && -> ||, would incorrectly enter varbit branch.
        let old = TypeName::simple("text");
        let new = TypeName::with_modifiers("varbit", vec![16]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    // Mutants #6, #8, #9: line 70 && -> || in timestamp check,
    // line 94 is_timestamp_type -> true, line 99 is_timestamptz_type -> true.
    #[test]
    fn test_timestamptz_to_timestamp_unsafe() {
        // Reverse direction: timestamptz -> timestamp is NOT safe.
        // With && -> ||, one side being timestamptz would incorrectly return Info.
        // Also kills is_timestamptz_type -> true on the old side (old is timestamptz,
        // is_timestamp_type(old) is false, but if is_timestamp_type always returned true
        // this would incorrectly match).
        let old = TypeName::simple("timestamptz");
        let new = TypeName::simple("timestamp");
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    #[test]
    fn test_timestamp_to_text_unsafe() {
        // old is timestamp but new is NOT timestamptz.
        // With && -> ||, is_timestamp_type(old) alone would match.
        // Also kills is_timestamptz_type -> true (would incorrectly match "text").
        let old = TypeName::simple("timestamp");
        let new = TypeName::simple("text");
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    #[test]
    fn test_text_to_timestamptz_unsafe() {
        // old is NOT timestamp, new IS timestamptz.
        // With && -> ||, is_timestamptz_type(new) alone would match -> Info.
        // Also kills is_timestamp_type -> true (would incorrectly match "text").
        let old = TypeName::simple("text");
        let new = TypeName::simple("timestamptz");
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    #[test]
    fn test_timestamp_without_tz_to_timestamptz_info() {
        // Tests the long-form name for timestamp.
        // Kills is_timestamp_type -> true if falsely matching non-timestamp types.
        let old = TypeName::simple("timestamp without time zone");
        let new = TypeName::simple("timestamp with time zone");
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Info);
    }

    #[test]
    fn test_integer_is_not_timestamp_type() {
        // integer -> timestamptz should be Unsafe, not Info.
        // Kills is_timestamp_type -> true (integer would match as timestamp).
        let old = TypeName::simple("integer");
        let new = TypeName::simple("timestamptz");
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    #[test]
    fn test_timestamp_to_integer_not_timestamptz() {
        // timestamp -> integer should be Unsafe.
        // Kills is_timestamptz_type -> true (integer would match as timestamptz).
        let old = TypeName::simple("timestamp");
        let new = TypeName::simple("integer");
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    // --- Mutation-killing tests for normalize_numeric_modifiers ---

    // Mutants #10, #11: line 127 delete - in (-1, -1) sentinel.
    // With the mutation, empty modifiers return (1, 1) instead of (-1, -1).
    // This sentinel is used in check_numeric_widening when both sides have
    // modifiers but the input is unexpected (e.g., 3+ modifiers).
    #[test]
    fn test_normalize_numeric_modifiers_empty() {
        // The _ branch should return (-1, -1) for empty modifiers.
        // If the `-` sign is deleted, it returns (1, 1) instead.
        assert_eq!(normalize_numeric_modifiers(&[]), (-1, -1));
    }

    #[test]
    fn test_normalize_numeric_modifiers_single() {
        assert_eq!(normalize_numeric_modifiers(&[10]), (10, 0));
    }

    #[test]
    fn test_normalize_numeric_modifiers_double() {
        assert_eq!(normalize_numeric_modifiers(&[10, 2]), (10, 2));
    }

    #[test]
    fn test_normalize_numeric_modifiers_triple_falls_to_sentinel() {
        // Three modifiers should hit the _ arm and return (-1, -1).
        assert_eq!(normalize_numeric_modifiers(&[1, 2, 3]), (-1, -1));
    }

    // This exercises the sentinel path through is_safe_cast: if the sentinel
    // becomes (1, 1), then numeric with 3 mods -> numeric(1, 1) would be Safe
    // instead of Unsafe, because (1, 1) == (1, 1).
    #[test]
    fn test_numeric_weird_modifiers_to_numeric_1_1_unsafe() {
        // numeric with 3 modifiers (normalizes to sentinel -1, -1)
        // -> numeric(1, 1). Should be Unsafe because sentinels don't match.
        // If sentinel were (1, 1) instead of (-1, -1), this would be Safe.
        let old = TypeName {
            name: "numeric".to_string(),
            modifiers: vec![1, 2, 3],
        };
        let new = TypeName::with_modifiers("numeric", vec![1, 1]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }

    // --- Direct tests for type-classification helpers ---

    #[test]
    fn test_is_varbit_type_positive() {
        assert!(is_varbit_type("varbit"));
        assert!(is_varbit_type("bit varying"));
    }

    #[test]
    fn test_is_varbit_type_negative() {
        assert!(!is_varbit_type("bit"));
        assert!(!is_varbit_type("text"));
        assert!(!is_varbit_type("varchar"));
    }

    #[test]
    fn test_is_timestamp_type_positive() {
        assert!(is_timestamp_type("timestamp"));
        assert!(is_timestamp_type("timestamp without time zone"));
    }

    #[test]
    fn test_is_timestamp_type_negative() {
        assert!(!is_timestamp_type("timestamptz"));
        assert!(!is_timestamp_type("timestamp with time zone"));
        assert!(!is_timestamp_type("integer"));
        assert!(!is_timestamp_type("text"));
    }

    #[test]
    fn test_is_timestamptz_type_positive() {
        assert!(is_timestamptz_type("timestamptz"));
        assert!(is_timestamptz_type("timestamp with time zone"));
    }

    #[test]
    fn test_is_timestamptz_type_negative() {
        assert!(!is_timestamptz_type("timestamp"));
        assert!(!is_timestamptz_type("timestamp without time zone"));
        assert!(!is_timestamptz_type("integer"));
        assert!(!is_timestamptz_type("text"));
    }

    // Test that decimal (alias for numeric) also works through is_safe_cast.
    #[test]
    fn test_decimal_widening_safe() {
        let old = TypeName::with_modifiers("decimal", vec![10, 2]);
        let new = TypeName::with_modifiers("decimal", vec![14, 2]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Safe);
    }

    #[test]
    fn test_decimal_to_numeric_widening_safe() {
        // Cross-alias: decimal -> numeric widening
        let old = TypeName::with_modifiers("decimal", vec![10, 2]);
        let new = TypeName::with_modifiers("numeric", vec![14, 2]);
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Safe);
    }

    #[test]
    fn test_decimal_to_text_unsafe() {
        // decimal is numeric type but text is not — must be Unsafe.
        // With && -> || on line 55, one side matching would incorrectly enter numeric branch.
        let old = TypeName::with_modifiers("decimal", vec![10, 2]);
        let new = TypeName::simple("text");
        assert_eq!(is_safe_cast(&old, &new), CastSafety::Unsafe);
    }
}
