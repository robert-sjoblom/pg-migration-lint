//! Statements PostgreSQL rejects against the schema the catalog holds just
//! before them. PGM027 reports them, and PGM504 leaves a rejected
//! `RENAME TO` to it.
//!
//! Each statement is judged against `catalog_before` with the unit's earlier
//! statements replayed on top ([`map_with_state_before`]), so a name an
//! earlier statement in the same unit frees or takes counts. Only positive
//! evidence rejects a statement: an object the catalog does not have may
//! exist outside the tracked migrations.

use crate::catalog::Catalog;
use crate::catalog::replay::map_with_state_before;
use crate::catalog::types::{NamedRelation, RenameClash};
use crate::names::{RelationName, ResolvedName};
use crate::parser::ir::{AlterTable, IrNode, Located};
use crate::rules::using_index::{UsingIndex, has_using_index, using_index_constraints};

/// Why PostgreSQL rejects a statement, with the names a message needs. Table
/// names are display names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Rejection {
    /// `ADD PRIMARY KEY` or `ADD UNIQUE ... USING INDEX index` on `table`,
    /// where `index` is in `table`'s schema but belongs to `owner`
    /// (`verify/tests/other/pgm027_using_index_of_another_table.sql`).
    UsingIndexOfAnotherTable {
        constraint_kind: &'static str,
        index: String,
        table: String,
        owner: String,
    },
    /// A rename onto a name that is taken where the renamed object needs it
    /// free (`verify/tests/naming/rename_taken.sql`).
    RenameOntoTakenName {
        renamed: Renamed,
        new_name: String,
        taken_by: TakenBy,
    },
    /// `DROP INDEX index` where `index` backs `constraint` on `table`
    /// (`verify/tests/other/pgm027_drop_constraint_index.sql`).
    DropIndexOfConstraint {
        index: String,
        constraint: String,
        table: String,
    },
}

/// The object a rejected rename renames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Renamed {
    /// `ALTER TABLE table RENAME TO`.
    Table { table: String },
    /// `ALTER INDEX index RENAME TO`.
    Index { index: String },
    /// `ALTER TABLE table RENAME CONSTRAINT constraint TO`.
    Constraint { constraint: String, table: String },
}

/// What already has the name a rejected rename wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TakenBy {
    /// The table `table`.
    Table { table: String },
    /// An index of that name on `table`.
    Index { table: String },
    /// A constraint of that name on `table`, the renamed object's table.
    Constraint { table: String },
}

/// For each of `statements`, why PostgreSQL rejects it, judged against
/// `catalog_before` with the earlier statements of `statements` replayed on
/// top. `None` when the catalog gives no reason to.
pub(super) fn rejections(
    catalog_before: &Catalog,
    statements: &[Located<IrNode>],
) -> Vec<Option<Rejection>> {
    if !statements.iter().any(|stmt| can_be_rejected(&stmt.node)) {
        return vec![None; statements.len()];
    }
    map_with_state_before(catalog_before, statements, |state, stmt| {
        rejection(state, &stmt.node)
    })
}

/// True for the statements [`rejection`] can reject, so a unit without any
/// needs no replay.
fn can_be_rejected(node: &IrNode) -> bool {
    match node {
        IrNode::RenameTable { .. }
        | IrNode::RenameIndex { .. }
        | IrNode::RenameConstraint { .. }
        | IrNode::DropIndex(_) => true,
        _ => has_using_index(node),
    }
}

/// Why PostgreSQL rejects `node` on top of `state`, if the catalog shows it.
fn rejection(state: &Catalog, node: &IrNode) -> Option<Rejection> {
    match node {
        IrNode::AlterTable(at) => using_index_of_another_table(state, at),
        IrNode::RenameTable { name, new_name } => {
            let clash = state.table_rename_clash(name.relation(), new_name)?;
            rename_onto_taken_name(
                state,
                Renamed::Table {
                    table: name.display_name(),
                },
                name.relation(),
                new_name,
                clash,
            )
        }
        IrNode::RenameIndex { index, new_name } => {
            let clash = state.index_rename_clash(index.relation(), new_name)?;
            let table = state.table_for_index(index.relation())?;
            rename_onto_taken_name(
                state,
                Renamed::Index {
                    index: index.display_name(),
                },
                table,
                new_name,
                clash,
            )
        }
        IrNode::RenameConstraint {
            table,
            old_name,
            new_name,
        } => {
            let clash = state.constraint_rename_clash(table.relation(), old_name, new_name)?;
            rename_onto_taken_name(
                state,
                Renamed::Constraint {
                    constraint: old_name.clone(),
                    table: table.display_name(),
                },
                table.relation(),
                new_name,
                clash,
            )
        }
        IrNode::DropIndex(di) => drop_index_of_constraint(state, &di.index_name),
        _ => None,
    }
}

/// The first `USING INDEX` in `at` that names an index of another table, as
/// [`UsingIndex::in_catalog`] tells it for PGM016, PGM017 and PGM107 too.
fn using_index_of_another_table(state: &Catalog, at: &AlterTable) -> Option<Rejection> {
    using_index_constraints(at).find_map(|(constraint_kind, index)| {
        let UsingIndex::OfAnotherTable { owner } =
            UsingIndex::in_catalog(state, at.name.relation(), index)
        else {
            return None;
        };
        Some(Rejection::UsingIndexOfAnotherTable {
            constraint_kind,
            index: index.to_string(),
            table: at.name.display_name(),
            owner,
        })
    })
}

/// The rejection of a rename of `renamed` to `new_name`, which `clash`
/// says is taken. `table` is the table the renamed object is or belongs to:
/// a relation name is looked up in its schema, and a constraint name on it.
fn rename_onto_taken_name(
    state: &Catalog,
    renamed: Renamed,
    table: &RelationName,
    new_name: &str,
    clash: RenameClash,
) -> Option<Rejection> {
    let taken_by = match clash {
        RenameClash::Relation => match state.relation_named(&table.in_same_schema(new_name))? {
            NamedRelation::Table(holder) => TakenBy::Table {
                table: holder.display_name.clone(),
            },
            NamedRelation::Index { table: holder } => TakenBy::Index {
                table: holder.display_name.clone(),
            },
        },
        RenameClash::Constraint => TakenBy::Constraint {
            table: state.get_table(table)?.display_name.clone(),
        },
    };
    Some(Rejection::RenameOntoTakenName {
        renamed,
        new_name: new_name.to_string(),
        taken_by,
    })
}

/// The rejection of `DROP INDEX index` when `index` backs a PRIMARY KEY,
/// UNIQUE or EXCLUDE constraint of its table. Only a registered index
/// counts, so an EXCLUDE constraint's, which the catalog does not register,
/// does not yet.
fn drop_index_of_constraint(state: &Catalog, index: &ResolvedName) -> Option<Rejection> {
    let table = state.get_table(state.table_for_index(index.relation())?)?;
    let constraint = table.constraint_backed_by(index.name())?;
    Some(Rejection::DropIndexOfConstraint {
        index: index.display_name(),
        constraint: constraint.name().to_string(),
        table: table.display_name.clone(),
    })
}
