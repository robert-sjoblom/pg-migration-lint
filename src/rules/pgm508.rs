#![doc = include_str!("docs/pgm508.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str =
    "Duplicate or redundant index detected (prefix of another index)";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm508.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Info;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        let IrNode::CreateIndex(ci) = &stmt.node else {
            continue;
        };

        let table = match ctx.catalog_after.get_table(ci.table_name.catalog_key()) {
            Some(t) => t,
            None => continue,
        };

        let Some(index_name) = &ci.index_name else {
            continue;
        };

        // find the index in catalog_after
        let new_idx = match table.indexes.iter().find(|idx| idx.name == *index_name) {
            Some(idx) => idx,
            None => continue,
        };

        // Skip if new index has expressions or is partial — these are specialized
        if new_idx.has_expressions() || new_idx.is_partial() {
            continue;
        }

        let new_cols: Vec<&str> = new_idx.column_names().collect();

        for other_idx in &table.indexes {
            if other_idx.name == new_idx.name {
                continue;
            }

            // Skip if other index has expressions or is partial
            if other_idx.has_expressions() || other_idx.is_partial() {
                continue;
            }

            // Skip if different access methods
            if new_idx.access_method != other_idx.access_method {
                continue;
            }

            let other_cols: Vec<&str> = other_idx.column_names().collect();

            if new_cols == other_cols {
                // Exact duplicate
                findings.push(rule.make_finding(
                    format!(
                        "Index '{}' on '{}' ({}) is an exact duplicate of index '{}'.",
                        index_name,
                        table.display_name,
                        new_cols.join(", "),
                        other_idx.name,
                    ),
                    ctx.file,
                    &stmt.span,
                ));
            } else if is_prefix(&new_cols, &other_cols) && !new_idx.unique {
                // New index is a prefix of existing — new is redundant
                findings.push(rule.make_finding(
                    format!(
                        "Index '{}' on '{}' ({}) is redundant — \
                         index '{}' ({}) covers the same prefix.",
                        index_name,
                        table.display_name,
                        new_cols.join(", "),
                        other_idx.name,
                        other_cols.join(", "),
                    ),
                    ctx.file,
                    &stmt.span,
                ));
            } else if is_prefix(&other_cols, &new_cols) && !other_idx.unique {
                // Existing index is a prefix of new — existing made redundant
                findings.push(rule.make_finding(
                    format!(
                        "Index '{}' on '{}' ({}) makes existing index '{}' ({}) redundant — \
                         the new index covers the same prefix.",
                        index_name,
                        table.display_name,
                        new_cols.join(", "),
                        other_idx.name,
                        other_cols.join(", "),
                    ),
                    ctx.file,
                    &stmt.span,
                ));
            }
        }
    }

    findings
}

/// Returns true if `shorter` is a non-empty strict prefix of `longer`.
fn is_prefix(shorter: &[&str], longer: &[&str]) -> bool {
    shorter.len() < longer.len() && shorter.iter().zip(longer.iter()).all(|(a, b)| a == b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{lint_ctx, located};

    fn create_index_stmt(name: &str, table: &str, columns: &[&str]) -> Located<IrNode> {
        located(IrNode::CreateIndex(CreateIndex {
            index_name: Some(name.to_string()),
            table_name: QualifiedName::unqualified(table),
            columns: columns
                .iter()
                .map(|c| IndexColumn::Column(c.to_string()))
                .collect(),
            unique: false,
            concurrent: false,
            if_not_exists: false,
            where_clause: None,
            only: false,
            access_method: "btree".to_string(),
        }))
    }

    #[test]
    fn test_exact_duplicate_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("email", "text", false)
                    .index("idx_a", &["email"], false)
                    .index("idx_b", &["email"], false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![create_index_stmt("idx_b", "orders", &["email"])];
        let findings = RuleId::Pgm508.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_prefix_new_is_shorter_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("a", "integer", false)
                    .column("b", "integer", false)
                    .index("idx_a", &["a"], false)
                    .index("idx_ab", &["a", "b"], false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![create_index_stmt("idx_a", "orders", &["a"])];
        let findings = RuleId::Pgm508.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_prefix_new_makes_existing_redundant_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("a", "integer", false)
                    .column("b", "integer", false)
                    .index("idx_a", &["a"], false)
                    .index("idx_ab", &["a", "b"], false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![create_index_stmt("idx_ab", "orders", &["a", "b"])];
        let findings = RuleId::Pgm508.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_different_access_methods_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("data", "jsonb", false)
                    .index("idx_btree", &["data"], false)
                    .index_with_method("idx_gin", &["data"], false, "gin");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![create_index_stmt("idx_btree", "orders", &["data"])];
        let findings = RuleId::Pgm508.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_shorter_is_unique_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("a", "integer", false)
                    .column("b", "integer", false)
                    .index("idx_a_unique", &["a"], true) // UNIQUE
                    .index("idx_ab", &["a", "b"], false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        // The unique index on (a) is the one being created — it's unique, so
        // even though (a) is a prefix of (a, b), it shouldn't fire.
        let stmts = vec![{
            let mut s = create_index_stmt("idx_a_unique", "orders", &["a"]);
            if let IrNode::CreateIndex(ref mut ci) = s.node {
                ci.unique = true;
            }
            s
        }];
        let findings = RuleId::Pgm508.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_partial_index_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("a", "integer", false)
                    .index("idx_a", &["a"], false)
                    .partial_index("idx_a_active", &["a"], false, "active = true");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![create_index_stmt("idx_a", "orders", &["a"])];
        let findings = RuleId::Pgm508.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_expression_index_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("email", "text", false)
                    .index("idx_email", &["email"], false)
                    .expression_index("idx_email_lower", &["expr:lower(email)"], false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![create_index_stmt("idx_email", "orders", &["email"])];
        let findings = RuleId::Pgm508.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_different_column_order_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("a", "integer", false)
                    .column("b", "integer", false)
                    .index("idx_ab", &["a", "b"], false)
                    .index("idx_ba", &["b", "a"], false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![create_index_stmt("idx_ab", "orders", &["a", "b"])];
        let findings = RuleId::Pgm508.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_new_plain_index_prefix_of_unique_backed_index_fires() {
        // A UNIQUE constraint's backing index is unique but that must not stop
        // it from being the *covering* side — only from being flagged redundant.
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("a", "integer", false)
                    .column("b", "integer", false)
                    .index("uq_orders_a_b", &["a", "b"], true) // UNIQUE-backed
                    .index("idx_a", &["a"], false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![create_index_stmt("idx_a", "orders", &["a"])];
        let findings = RuleId::Pgm508.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_new_plain_index_exact_duplicate_of_unique_backed_index_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("email", "text", false)
                    .index("uq_orders_email", &["email"], true) // UNIQUE-backed
                    .index("idx_email", &["email"], false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![create_index_stmt("idx_email", "orders", &["email"])];
        let findings = RuleId::Pgm508.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_no_overlap_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("a", "integer", false)
                    .column("b", "integer", false)
                    .index("idx_a", &["a"], false)
                    .index("idx_b", &["b"], false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![create_index_stmt("idx_a", "orders", &["a"])];
        let findings = RuleId::Pgm508.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
