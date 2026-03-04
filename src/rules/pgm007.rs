//! PGM007 — `ALTER COLUMN TYPE` on existing table
//!
//! Detects `ALTER TABLE ... ALTER COLUMN ... TYPE ...` on tables that already
//! exist in the catalog. Most type changes require a full table rewrite under
//! an `ACCESS EXCLUSIVE` lock. A hardcoded allowlist of safe (binary-coercible)
//! casts suppresses the finding for known safe conversions.

use crate::parser::ir::{AlterTableAction, IrNode, Located, TypeName};
use crate::rules::{Finding, LintContext, Rule, Severity, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str = "ALTER COLUMN TYPE on existing table causes table rewrite";

pub(super) const EXPLAIN: &str = "PGM007 — ALTER COLUMN TYPE on existing table\n\
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
         - varbit(N) -> varbit(M) where M > N\n\
         \n\
         INFO cast:\n\
         - timestamp -> timestamptz (no rewrite in PG 9.2+; the cast uses the\n\
           session TimeZone at ALTER time, so verify that the executing session\n\
           has TimeZone=UTC — a server default of UTC is not sufficient if the\n\
           connection overrides it)\n\
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

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Critical;

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

/// For types with a single modifier (e.g., varchar(N), varbit(N)):
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
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{located, make_ctx};
    use rstest::rstest;
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[rstest]
    #[case::varchar_widening_safe("varchar", &[50], "varchar", &[100], CastSafety::Safe)]
    #[case::varchar_to_text_safe("varchar", &[50], "text", &[], CastSafety::Safe)]
    #[case::varchar_narrowing_unsafe("varchar", &[100], "varchar", &[50], CastSafety::Unsafe)]
    #[case::text_to_varchar_unsafe("text", &[], "varchar", &[100], CastSafety::Unsafe)]
    #[case::numeric_widening_same_scale_safe("numeric", &[10, 2], "numeric", &[12, 2], CastSafety::Safe)]
    #[case::numeric_precision_only_to_precision_scale_safe("numeric", &[10], "numeric", &[12, 0], CastSafety::Safe)]
    #[case::numeric_precision_scale_to_precision_only_safe("numeric", &[10, 0], "numeric", &[12], CastSafety::Safe)]
    #[case::numeric_identity_after_normalization("numeric", &[10], "numeric", &[10, 0], CastSafety::Safe)]
    #[case::numeric_narrowing_single_modifier_unsafe("numeric", &[10], "numeric", &[8], CastSafety::Unsafe)]
    #[case::numeric_scale_change_via_normalization_unsafe("numeric", &[10], "numeric", &[10, 2], CastSafety::Unsafe)]
    #[case::numeric_bare_to_bare_safe("numeric", &[], "numeric", &[], CastSafety::Safe)]
    #[case::numeric_bare_to_constrained_unsafe("numeric", &[], "numeric", &[10], CastSafety::Unsafe)]
    #[case::numeric_constrained_to_bare_safe("numeric", &[10, 2], "numeric", &[], CastSafety::Safe)]
    #[case::numeric_scale_change_unsafe("numeric", &[10, 2], "numeric", &[10, 4], CastSafety::Unsafe)]
    #[case::numeric_to_text_unsafe("numeric", &[10, 2], "text", &[], CastSafety::Unsafe)]
    #[case::text_to_numeric_unsafe("text", &[], "numeric", &[10, 2], CastSafety::Unsafe)]
    #[case::decimal_widening_safe("decimal", &[10, 2], "decimal", &[14, 2], CastSafety::Safe)]
    #[case::decimal_to_numeric_widening_safe("decimal", &[10, 2], "numeric", &[14, 2], CastSafety::Safe)]
    #[case::decimal_to_text_unsafe("decimal", &[10, 2], "text", &[], CastSafety::Unsafe)]
    // numeric with weird modifiers (sentinel path)
    #[case::numeric_weird_modifiers_to_numeric_1_1_unsafe("numeric", &[1, 2, 3], "numeric", &[1, 1], CastSafety::Unsafe)]
    #[case::int_to_bigint_unsafe("integer", &[], "bigint", &[], CastSafety::Unsafe)]
    #[case::totally_different_types_unsafe("integer", &[], "text", &[], CastSafety::Unsafe)]
    #[case::timestamp_to_timestamptz_info("timestamp", &[], "timestamptz", &[], CastSafety::Info)]
    #[case::timestamp_without_tz_to_timestamptz_info("timestamp without time zone", &[], "timestamp with time zone", &[], CastSafety::Info)]
    #[case::timestamptz_to_timestamp_unsafe("timestamptz", &[], "timestamp", &[], CastSafety::Unsafe)]
    #[case::timestamp_to_text_unsafe("timestamp", &[], "text", &[], CastSafety::Unsafe)]
    #[case::text_to_timestamptz_unsafe("text", &[], "timestamptz", &[], CastSafety::Unsafe)]
    #[case::integer_is_not_timestamp_type("integer", &[], "timestamptz", &[], CastSafety::Unsafe)]
    #[case::timestamp_to_integer_not_timestamptz("timestamp", &[], "integer", &[], CastSafety::Unsafe)]
    #[case::bit_widening_unsafe("bit", &[8], "bit", &[16], CastSafety::Unsafe)]
    #[case::bit_narrowing_unsafe("bit", &[16], "bit", &[8], CastSafety::Unsafe)]
    #[case::bit_to_varbit_unsafe("bit", &[8], "varbit", &[16], CastSafety::Unsafe)]
    #[case::varbit_to_bit_unsafe("varbit", &[16], "bit", &[8], CastSafety::Unsafe)]
    #[case::varbit_widening_safe("varbit", &[8], "varbit", &[16], CastSafety::Safe)]
    #[case::bit_varying_widening_safe("bit varying", &[8], "bit varying", &[16], CastSafety::Safe)]
    #[case::varbit_narrowing_unsafe("varbit", &[16], "varbit", &[8], CastSafety::Unsafe)]
    #[case::varbit_to_text_unsafe("varbit", &[8], "text", &[], CastSafety::Unsafe)]
    #[case::text_to_varbit_unsafe("text", &[], "varbit", &[16], CastSafety::Unsafe)]
    fn test_is_safe_cast(
        #[case] old_name: &str,
        #[case] old_mods: &[i64],
        #[case] new_name: &str,
        #[case] new_mods: &[i64],
        #[case] expected: CastSafety,
    ) {
        let old = TypeName::with_modifiers(old_name, old_mods.to_vec());
        let new = TypeName::with_modifiers(new_name, new_mods.to_vec());
        assert_eq!(is_safe_cast(&old, &new), expected);
    }

    #[rstest]
    #[case::normalize_numeric_modifiers_empty(&[], (-1, -1))]
    #[case::normalize_numeric_modifiers_single(&[10], (10, 0))]
    #[case::normalize_numeric_modifiers_double(&[10, 2], (10, 2))]
    #[case::normalize_numeric_modifiers_triple_falls_to_sentinel(&[1, 2, 3], (-1, -1))]
    fn test_normalize_numeric_modifiers(#[case] input: &[i64], #[case] expected: (i64, i64)) {
        assert_eq!(normalize_numeric_modifiers(input), expected);
    }

    #[rstest]
    #[case::varbit_positive(is_varbit_type, "varbit", true)]
    #[case::bit_varying_positive(is_varbit_type, "bit varying", true)]
    #[case::bit_not_varbit(is_varbit_type, "bit", false)]
    #[case::text_not_varbit(is_varbit_type, "text", false)]
    #[case::varchar_not_varbit(is_varbit_type, "varchar", false)]
    #[case::timestamp_positive(is_timestamp_type, "timestamp", true)]
    #[case::timestamp_without_tz_positive(is_timestamp_type, "timestamp without time zone", true)]
    #[case::timestamptz_not_timestamp(is_timestamp_type, "timestamptz", false)]
    #[case::timestamp_with_tz_not_timestamp(is_timestamp_type, "timestamp with time zone", false)]
    #[case::integer_not_timestamp(is_timestamp_type, "integer", false)]
    #[case::text_not_timestamp(is_timestamp_type, "text", false)]
    #[case::timestamptz_positive(is_timestamptz_type, "timestamptz", true)]
    #[case::timestamp_with_tz_positive(is_timestamptz_type, "timestamp with time zone", true)]
    #[case::timestamp_not_timestamptz(is_timestamptz_type, "timestamp", false)]
    #[case::timestamp_without_tz_not_timestamptz(
        is_timestamptz_type,
        "timestamp without time zone",
        false
    )]
    #[case::integer_not_timestamptz(is_timestamptz_type, "integer", false)]
    #[case::text_not_timestamptz(is_timestamptz_type, "text", false)]
    fn test_type_classifier(
        #[case] classifier: fn(&str) -> bool,
        #[case] input: &str,
        #[case] expected: bool,
    ) {
        assert_eq!(classifier(input), expected);
    }

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

        let findings = RuleId::Pgm007.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_varchar_widening_with_old_type_directive_in_ir() {
        // Test when old_type is provided in the IR node (not from catalog)
        let before = CatalogBuilder::new()
            .table("users", |t| {
                t.column("name", "varchar", false).pk(&["name"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("users"),
            actions: vec![AlterTableAction::AlterColumnType {
                column_name: "name".to_string(),
                new_type: TypeName::with_modifiers("varchar", vec![100]),
                old_type: Some(TypeName::with_modifiers("varchar", vec![50])),
            }],
        }))];

        let findings = RuleId::Pgm007.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "Widening varchar should be safe even when old_type is provided"
        );
    }

    #[test]
    fn test_alter_column_type_column_not_in_catalog() {
        // Column doesn't exist in catalog — should assume unsafe
        let before = CatalogBuilder::new()
            .table("users", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("users"),
            actions: vec![AlterTableAction::AlterColumnType {
                column_name: "nonexistent".to_string(),
                new_type: TypeName::simple("text"),
                old_type: None,
            }],
        }))];

        let findings = RuleId::Pgm007.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Critical);
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

        let findings = RuleId::Pgm007.check(&stmts, &ctx);
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

        let findings = RuleId::Pgm007.check(&stmts, &ctx);
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

        let findings = RuleId::Pgm007.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
