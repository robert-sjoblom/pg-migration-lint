//! Shared column type checking logic for rules that flag specific column types.
//!
//! Used by PGM101-104, which all follow the same pattern: flag columns whose type
//! matches a predicate, across `CreateTable`, `AddColumn`, and `AlterColumnType`.

use crate::parser::ir::{AlterTableAction, IrNode, Located, QualifiedName, TypeName};
use crate::rules::{Finding, LintContext, Severity};

/// Check all columns in CREATE TABLE and ALTER TABLE statements against a type predicate.
///
/// For each column whose type matches `predicate`, a finding is emitted with a message
/// produced by `message_fn(column_name, table_name, type_name)`.
pub fn check_column_types(
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
    rule_id: &str,
    severity: Severity,
    predicate: impl Fn(&TypeName) -> bool,
    message_fn: impl Fn(&str, &QualifiedName, &TypeName) -> String,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        match &stmt.node {
            IrNode::CreateTable(ct) => {
                for col in &ct.columns {
                    if predicate(&col.type_name) {
                        findings.push(Finding::new(
                            rule_id,
                            severity,
                            message_fn(&col.name, &ct.name, &col.type_name),
                            ctx.file,
                            &stmt.span,
                        ));
                    }
                }
            }
            IrNode::AlterTable(at) => {
                for action in &at.actions {
                    match action {
                        AlterTableAction::AddColumn(col) => {
                            if predicate(&col.type_name) {
                                findings.push(Finding::new(
                                    rule_id,
                                    severity,
                                    message_fn(&col.name, &at.name, &col.type_name),
                                    ctx.file,
                                    &stmt.span,
                                ));
                            }
                        }
                        AlterTableAction::AlterColumnType {
                            column_name,
                            new_type,
                            ..
                        } => {
                            if predicate(new_type) {
                                findings.push(Finding::new(
                                    rule_id,
                                    severity,
                                    message_fn(column_name, &at.name, new_type),
                                    ctx.file,
                                    &stmt.span,
                                ));
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    findings
}
