//! The names one statement gives the constraints it adds.
//!
//! Postgres names a statement's objects one at a time, in the order it
//! creates them, and each generated name also avoids every name created
//! before it in the same statement (`verify/tests/naming/collisions.sql`).
//! That order is not the textual one (`collision_order.sql`):
//!
//! - CREATE TABLE creates its CHECKs, then the PRIMARY KEY, then UNIQUE and
//!   EXCLUDE, then FOREIGN KEYs, each group in textual order.
//! - ALTER TABLE attaches `USING INDEX` constraints, then creates the other
//!   index-backed constraints, then CHECKs and FOREIGN KEYs together, each
//!   group in textual order. It also names the constraints of every
//!   `ADD COLUMN` before those of any `ADD CONSTRAINT`, which the IR cannot
//!   tell apart, so that part is not modelled.
//!
//! [`constraint_names`] chooses the names in creation order and hands them
//! back in textual order, so replay still adds the constraints in the order
//! they were written.

use std::collections::HashSet;

use crate::catalog::generated_names::{GeneratedName, TakenNames};
use crate::parser::ir::TableConstraint;

/// A statement that adds constraints. Each kind creates them in its own
/// order.
#[derive(Debug, Clone, Copy)]
pub(super) enum ConstraintStatement {
    CreateTable,
    AlterTable,
}

impl ConstraintStatement {
    /// Where `constraint` comes in the order this statement creates its
    /// constraints in, lowest first. Ties keep their textual order.
    fn creation_rank(self, constraint: &TableConstraint) -> u8 {
        use TableConstraint::{Check, Exclude, ForeignKey, PrimaryKey, Unique};
        match (self, constraint) {
            (Self::CreateTable, Check { .. }) => 0,
            (Self::CreateTable, PrimaryKey { .. }) => 1,
            (Self::CreateTable, Unique { .. } | Exclude { .. }) => 2,
            (Self::CreateTable, ForeignKey { .. }) => 3,
            (
                Self::AlterTable,
                PrimaryKey {
                    using_index: Some(_),
                    ..
                }
                | Unique {
                    using_index: Some(_),
                    ..
                },
            ) => 0,
            (Self::AlterTable, PrimaryKey { .. } | Unique { .. } | Exclude { .. }) => 1,
            (Self::AlterTable, Check { .. } | ForeignKey { .. }) => 2,
        }
    }
}

/// The name of each constraint in `constraints`, which one `statement` adds
/// to the table with bare name `table`, in the same order. A constraint
/// keeps its own name, a `USING INDEX` constraint takes the index's, and any
/// other gets the name Postgres generates, unique against `schema` and the
/// names chosen before it.
pub(super) fn constraint_names(
    statement: ConstraintStatement,
    table: &str,
    constraints: &[&TableConstraint],
    schema: impl TakenNames,
) -> Vec<String> {
    let mut creation_order: Vec<usize> = (0..constraints.len()).collect();
    creation_order.sort_by_key(|&i| statement.creation_rank(constraints[i]));

    let mut taken = StatementNames::new(schema, table);
    let mut names = vec![String::new(); constraints.len()];
    for i in creation_order {
        names[i] = taken.name(table, constraints[i]);
    }
    names
}

/// The names a statement has used so far, on top of what its schema holds.
struct StatementNames<S> {
    schema: S,
    constraints: HashSet<String>,
    relations: HashSet<String>,
    generated_primary_key: Option<String>,
}

impl<S: TakenNames> StatementNames<S> {
    /// Nothing used yet but `table` itself, which a CREATE TABLE adds to the
    /// schema before any of its constraints.
    fn new(schema: S, table: &str) -> Self {
        Self {
            schema,
            constraints: HashSet::new(),
            relations: HashSet::from([table.to_string()]),
            generated_primary_key: None,
        }
    }

    /// Name `constraint` and record the name as used: as a constraint, and
    /// also as a relation when an index of the same name backs it.
    fn name(&mut self, table: &str, constraint: &TableConstraint) -> String {
        let name = match constraint {
            TableConstraint::PrimaryKey {
                name: Some(name), ..
            }
            | TableConstraint::Unique {
                name: Some(name), ..
            }
            | TableConstraint::ForeignKey {
                name: Some(name), ..
            }
            | TableConstraint::Check {
                name: Some(name), ..
            }
            | TableConstraint::Exclude {
                name: Some(name), ..
            }
            | TableConstraint::PrimaryKey {
                using_index: Some(name),
                ..
            }
            | TableConstraint::Unique {
                using_index: Some(name),
                ..
            } => name.clone(),
            TableConstraint::PrimaryKey { .. } => self.generated_primary_key(table),
            TableConstraint::Unique {
                columns,
                include_columns,
                ..
            } => GeneratedName::unique(table, columns, include_columns).choose(self),
            TableConstraint::ForeignKey { columns, .. } => {
                GeneratedName::foreign_key(table, columns).choose(self)
            }
            TableConstraint::Check {
                referenced_columns, ..
            } => GeneratedName::check(table, referenced_columns).choose(self),
            TableConstraint::Exclude {
                elements,
                include_columns,
                ..
            } => GeneratedName::exclude(table, elements, include_columns).choose(self),
        };
        if matches!(
            constraint,
            TableConstraint::PrimaryKey { .. }
                | TableConstraint::Unique { .. }
                | TableConstraint::Exclude { .. }
        ) {
            self.relations.insert(name.clone());
        }
        self.constraints.insert(name.clone());
        name
    }

    /// A table has one primary key, so every unnamed PRIMARY KEY in a
    /// statement is the same key, with one generated name. The parser hands
    /// an inline `PRIMARY KEY` over twice (the inline-PK double registration
    /// in TODO.md), and the second copy must not get `_pkey1`.
    fn generated_primary_key(&mut self, table: &str) -> String {
        if let Some(name) = &self.generated_primary_key {
            return name.clone();
        }
        let name = GeneratedName::primary_key(table).choose(self);
        self.generated_primary_key = Some(name.clone());
        name
    }
}

impl<S: TakenNames> TakenNames for StatementNames<S> {
    fn constraint_taken(&self, name: &str) -> bool {
        self.constraints.contains(name) || self.schema.constraint_taken(name)
    }

    fn relation_taken(&self, name: &str) -> bool {
        self.relations.contains(name) || self.schema.relation_taken(name)
    }
}
