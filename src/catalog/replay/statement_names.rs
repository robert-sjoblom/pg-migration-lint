//! The names one statement gives the constraints it adds.
//!
//! Postgres names a statement's objects one at a time, in the order it
//! creates them, and each generated name also avoids every name created
//! before it in the same statement (`verify/tests/naming/collisions.sql`).
//! That order is not the textual one (`collision_order.sql`):
//!
//! - CREATE TABLE creates its CHECKs, then the PRIMARY KEY, then UNIQUE and
//!   EXCLUDE, then FOREIGN KEYs, each group in textual order.
//! - ALTER TABLE runs in passes: it attaches `USING INDEX` constraints, then
//!   creates the other index-backed constraints, then CHECKs and FOREIGN KEYs
//!   together. Within a pass it creates the constraints written inline in
//!   each `ADD COLUMN`, in the order of those subcommands, before those of
//!   any `ADD CONSTRAINT`, which keep their textual order. Within one
//!   `ADD COLUMN`, its CHECKs come before its FOREIGN KEYs.
//!
//! [`create_table_constraint_names`] and [`alter_table_constraint_names`]
//! choose the names in creation order and hand them back in textual order,
//! so replay still adds the constraints in the order they were written.

use std::collections::HashSet;

use crate::catalog::generated_names::{GeneratedName, TakenNames};
use crate::parser::ir::{AlterTableAction, ConstraintOrigin, TableConstraint};

/// When a CREATE TABLE creates a constraint, earliest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CreateTableRank {
    Check,
    PrimaryKey,
    UniqueOrExclude,
    ForeignKey,
}

impl CreateTableRank {
    fn of(constraint: &TableConstraint) -> Self {
        match constraint {
            TableConstraint::Check { .. } => Self::Check,
            TableConstraint::PrimaryKey { .. } => Self::PrimaryKey,
            TableConstraint::Unique { .. } | TableConstraint::Exclude { .. } => {
                Self::UniqueOrExclude
            }
            TableConstraint::ForeignKey { .. } => Self::ForeignKey,
        }
    }
}

/// When an ALTER TABLE creates a constraint, earliest first. The fields
/// compare in declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct AlterTableRank {
    pass: AlterTablePass,
    subcommand: SubcommandRank,
}

/// The ALTER TABLE pass that creates a constraint, in the order the passes
/// run (`AT_PASS_*` in `tablecmds.c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AlterTablePass {
    /// `USING INDEX` constraints, which attach an existing index.
    AddIndexConstraint,
    /// The other PRIMARY KEY, UNIQUE and EXCLUDE constraints, which build
    /// an index.
    AddIndex,
    /// CHECK and FOREIGN KEY constraints.
    AddOtherConstraint,
}

/// Where a constraint comes within its pass, by the subcommand that wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SubcommandRank {
    /// Written inline in the `position`th `ADD COLUMN` of the statement.
    /// Within one `ADD COLUMN`, a FOREIGN KEY comes after the CHECKs.
    AddColumn { position: usize, is_foreign_key: bool },
    /// Written as an `ADD CONSTRAINT`.
    AddConstraint,
}

impl AlterTableRank {
    /// The rank of `constraint`, written in the subcommand `origin`. An
    /// inline constraint belongs to the `ADD COLUMN` at `add_column_position`.
    fn of(
        constraint: &TableConstraint,
        origin: ConstraintOrigin,
        add_column_position: usize,
    ) -> Self {
        let pass = match constraint {
            TableConstraint::PrimaryKey {
                using_index: Some(_),
                ..
            }
            | TableConstraint::Unique {
                using_index: Some(_),
                ..
            } => AlterTablePass::AddIndexConstraint,
            TableConstraint::PrimaryKey { .. }
            | TableConstraint::Unique { .. }
            | TableConstraint::Exclude { .. } => AlterTablePass::AddIndex,
            TableConstraint::Check { .. } | TableConstraint::ForeignKey { .. } => {
                AlterTablePass::AddOtherConstraint
            }
        };
        let subcommand = match origin {
            ConstraintOrigin::AddColumn => SubcommandRank::AddColumn {
                position: add_column_position,
                is_foreign_key: matches!(constraint, TableConstraint::ForeignKey { .. }),
            },
            ConstraintOrigin::AddConstraint => SubcommandRank::AddConstraint,
        };
        Self { pass, subcommand }
    }
}

/// Each constraint of a CREATE TABLE of the table with bare name `table`,
/// in textual order, with its name. See [`name_in_creation_order`].
pub(super) fn create_table_constraint_names<'a>(
    table: &str,
    constraints: &'a [TableConstraint],
    schema: impl TakenNames,
) -> Vec<(&'a TableConstraint, String)> {
    let ranked: Vec<_> = constraints
        .iter()
        .map(|constraint| (constraint, CreateTableRank::of(constraint)))
        .collect();
    name_in_creation_order(table, ranked, schema)
}

/// Each constraint that the ALTER TABLE `actions` add to the table with bare
/// name `table`, in textual order, with its name. See
/// [`name_in_creation_order`].
pub(super) fn alter_table_constraint_names<'a>(
    table: &str,
    actions: &'a [AlterTableAction],
    schema: impl TakenNames,
) -> Vec<(&'a TableConstraint, String)> {
    let mut add_column_position = 0;
    let mut ranked = Vec::new();
    for action in actions {
        match action {
            AlterTableAction::AddColumn(_) => add_column_position += 1,
            AlterTableAction::AddConstraint { constraint, origin } => ranked.push((
                constraint,
                AlterTableRank::of(constraint, *origin, add_column_position),
            )),
            _ => {}
        }
    }
    name_in_creation_order(table, ranked, schema)
}

/// Name each constraint in `ranked`, which one statement adds to the table
/// with bare name `table`, lowest rank first and equal ranks in textual
/// order, and return them in textual order with their names. A constraint
/// keeps its own name, a `USING INDEX` constraint takes the index's, and any
/// other gets the name Postgres generates, unique against `schema` and the
/// names chosen before it.
fn name_in_creation_order<'a, R: Ord + Copy>(
    table: &str,
    ranked: Vec<(&'a TableConstraint, R)>,
    schema: impl TakenNames,
) -> Vec<(&'a TableConstraint, String)> {
    let mut creation_order: Vec<usize> = (0..ranked.len()).collect();
    creation_order.sort_by_key(|&i| ranked[i].1);

    let mut taken = StatementNames::new(schema, table);
    let mut names = vec![String::new(); ranked.len()];
    for i in creation_order {
        names[i] = taken.name(table, ranked[i].0);
    }
    ranked
        .into_iter()
        .map(|(constraint, _)| constraint)
        .zip(names)
        .collect()
}

/// The names a statement has used so far, on top of what its schema holds.
struct StatementNames<S> {
    schema: S,
    constraints: HashSet<String>,
    relations: HashSet<String>,
}

impl<S: TakenNames> StatementNames<S> {
    /// Nothing used yet but `table` itself, which a CREATE TABLE adds to the
    /// schema before any of its constraints.
    fn new(schema: S, table: &str) -> Self {
        Self {
            schema,
            constraints: HashSet::new(),
            relations: HashSet::from([table.to_string()]),
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
            TableConstraint::PrimaryKey { .. } => GeneratedName::primary_key(table).choose(self),
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
}

impl<S: TakenNames> TakenNames for StatementNames<S> {
    fn constraint_taken(&self, name: &str) -> bool {
        self.constraints.contains(name) || self.schema.constraint_taken(name)
    }

    fn relation_taken(&self, name: &str) -> bool {
        self.relations.contains(name) || self.schema.relation_taken(name)
    }
}
