//! PGM009 — `ALTER COLUMN TYPE` on existing table
//!
//! Detects `ALTER TABLE ... ALTER COLUMN ... TYPE ...` on tables that already
//! exist in the catalog. Most type changes require a full table rewrite under
//! an `ACCESS EXCLUSIVE` lock. A hardcoded allowlist of safe (binary-coercible)
//! casts suppresses the finding for known safe conversions.

use crate::parser::ir::{AlterTableAction, IrNode, Located, TypeName};
use crate::rules::{Finding, LintContext, Rule, Severity};

/// Rule that flags column type changes on existing tables.
pub struct Pgm009;

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

/// Check numeric(P,S) -> numeric(P2,S) widening.
/// Safe if: P2 >= P and scale is the same.
fn check_numeric_widening(old: &TypeName, new: &TypeName) -> CastSafety {
    match (old.modifiers.as_slice(), new.modifiers.as_slice()) {
        ([old_p, old_s], [new_p, new_s]) => {
            if new_p >= old_p && new_s == old_s {
                CastSafety::Safe
            } else {
                CastSafety::Unsafe
            }
        }
        // If modifiers don't match pattern, treat as potentially unsafe
        // numeric with no modifiers -> numeric with no modifiers is a no-op
        ([], []) => CastSafety::Safe,
        _ => CastSafety::Unsafe,
    }
}

impl Rule for Pgm009 {
    fn id(&self) -> &'static str {
        "PGM009"
    }

    fn default_severity(&self) -> Severity {
        Severity::Critical
    }

    fn description(&self) -> &'static str {
        "ALTER COLUMN TYPE on existing table causes table rewrite"
    }

    fn explain(&self) -> &'static str {
        "PGM009 — ALTER COLUMN TYPE on existing table\n\
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
           ALTER TABLE orders RENAME COLUMN amount_new TO amount;"
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        let mut findings = Vec::new();

        for stmt in statements {
            if let IrNode::AlterTable(ref at) = stmt.node {
                let table_key = at.name.catalog_key();

                // Only flag if table exists in catalog_before and is not newly created.
                if !ctx.is_existing_table(table_key) {
                    continue;
                }

                for action in &at.actions {
                    if let AlterTableAction::AlterColumnType {
                        column_name,
                        new_type,
                        old_type,
                    } = action
                    {
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
                            CastSafety::Safe => continue,
                            CastSafety::Info => Severity::Info,
                            CastSafety::Unsafe => self.default_severity(),
                        };

                        let old_display = resolved_old_type
                            .map(|t| t.to_string())
                            .unwrap_or_else(|| "unknown".to_string());

                        findings.push(Finding::new(
                            self.id(),
                            severity,
                            format!(
                                "Changing column type on existing table '{table}' \
                                 ('{col}': {old} \u{2192} {new}) rewrites the entire table \
                                 under an ACCESS EXCLUSIVE lock. For large tables, this causes \
                                 extended downtime. Consider creating a new column, backfilling, \
                                 and swapping instead.",
                                table = at.name,
                                col = column_name,
                                old = old_display,
                                new = new_type,
                            ),
                            ctx.file,
                            &stmt.span,
                        ));
                    }
                }
            }
        }

        findings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::test_helpers::{located, make_ctx};
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

        let findings = Pgm009.check(&stmts, &ctx);
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

        let findings = Pgm009.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Critical);
        assert!(findings[0].message.contains("orders"));
        assert!(findings[0].message.contains("amount"));
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

        let findings = Pgm009.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, Severity::Info);
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

        let findings = Pgm009.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
