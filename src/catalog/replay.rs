//! Catalog replay engine — applies IR nodes to mutate the catalog.
//!
//! The replay engine processes migration units in order, applying each
//! IR statement to build up the table catalog. This is the core of the
//! single-pass replay strategy: the pipeline calls [`apply`] for each
//! migration unit, and the catalog accumulates state over time.

use crate::catalog::generated_names::{GeneratedName, TakenNames};
use crate::catalog::types::*;
use crate::input::MigrationUnit;
use crate::names::{RelationName, ResolvedName};
use crate::parser::ir::*;
use statement_names::{alter_table_constraint_names, create_table_constraint_names};

mod statement_names;
#[cfg(test)]
mod tests;

/// Apply a single migration unit's IR nodes to mutate the catalog.
///
/// Called by the pipeline for each unit in order. Each statement in the
/// unit is applied sequentially. Statements that reference tables not
/// present in the catalog are silently skipped (the table may belong
/// to a different schema or be managed outside the tracked migrations).
pub(crate) fn apply(catalog: &mut Catalog, unit: &MigrationUnit) {
    for located in &unit.statements {
        if let Some(warning) = apply_node(catalog, &located.node) {
            eprintln!("warning: {warning}");
        }
    }
}

/// Apply a single IR node to the catalog. Returns a warning when the
/// statement contradicts what the catalog holds, for the caller to report.
fn apply_node(catalog: &mut Catalog, node: &IrNode) -> Option<String> {
    match node {
        IrNode::CreateTable(ct) => return apply_create_table(catalog, ct),
        IrNode::AlterTable(at) => apply_alter_table(catalog, at),
        IrNode::CreateIndex(ci) => return apply_create_index(catalog, ci),
        IrNode::DropIndex(di) => apply_drop_index(catalog, di),
        IrNode::DropTable(dt) => apply_drop_table(catalog, dt),
        IrNode::RenameTable { name, new_name } => apply_rename_table(catalog, name, new_name),
        IrNode::RenameColumn {
            table,
            old_name,
            new_name,
        } => apply_rename_column(catalog, table, old_name, new_name),
        IrNode::RenameIndex { index, new_name } => apply_rename_index(catalog, index, new_name),
        IrNode::RenameConstraint {
            table,
            old_name,
            new_name,
        } => apply_rename_constraint(catalog, table, old_name, new_name),
        IrNode::AlterIndexAttachPartition {
            parent_index_name, ..
        } => apply_alter_index_attach(catalog, parent_index_name.relation()),
        IrNode::DropSchema(ds) => apply_drop_schema(catalog, ds),
        IrNode::TruncateTable(_)
        | IrNode::Cluster(_)
        | IrNode::VacuumFull(_)
        | IrNode::Reindex(_) => { /* no schema state change */ }
        IrNode::InsertInto(_) | IrNode::UpdateTable(_) | IrNode::DeleteFrom(_) => {
            /* DML: no schema change */
        }
        IrNode::Unparseable { table_hint, .. } => apply_unparseable(catalog, table_hint.as_ref()),
        IrNode::Ignored { .. } => { /* no-op */ }
    }
    None
}

/// Handle CREATE TABLE: insert a new table into the catalog with columns,
/// constraints, and indexes derived from the statement.
///
/// When `IF NOT EXISTS` is used and the table already exists, the statement
/// is a no-op in PostgreSQL. We mirror that by keeping the existing catalog
/// state and returning a warning — the migration chain is ambiguous at that
/// point (which definition is the truth?). Replacing an existing table
/// returns a warning too.
fn apply_create_table(catalog: &mut Catalog, ct: &CreateTable) -> Option<String> {
    let table_key = ct.name.relation().clone();

    let mut warning = None;
    if catalog.has_table(&table_key) {
        if ct.if_not_exists {
            return Some(format!(
                "CREATE TABLE IF NOT EXISTS `{}` skipped — table already exists in catalog. \
                 The migration chain may be inconsistent.",
                ct.name.display_name()
            ));
        }
        warning = Some(format!(
            "CREATE TABLE `{}` overwrites existing table in catalog. \
             The table may have been dropped outside tracked migrations, or this is a duplicate definition.",
            ct.name.display_name()
        ));
        // The replaced table's indexes and constraints go with it, which
        // frees their names for the new table's.
        catalog.remove_table(&table_key);
    }

    let parent_key = ct.partition_of.as_ref().map(|p| p.relation().clone());

    let mut table = TableState {
        name: table_key.clone(),
        display_name: ct.name.display_name(),
        columns: Vec::new(),
        indexes: Vec::new(),
        constraints: Vec::new(),
        has_primary_key: false,
        incomplete: false,
        is_partitioned: ct.partition_by.is_some(),
        partition_by: ct.partition_by.as_ref().map(|pb| PartitionByInfo {
            strategy: pb.strategy,
            columns: pb.columns.clone(),
        }),
        parent_table: parent_key.clone(),
    };

    // For PARTITION OF, inherit columns from the parent table if it exists.
    if let Some(ref pk) = parent_key
        && let Some(parent) = catalog.get_table(pk)
    {
        for col in &parent.columns {
            table.columns.push(col.clone());
        }
    }

    // Explicit columns on the child, or regular table columns.
    table
        .columns
        .extend(ct.columns.iter().map(column_def_to_state));

    // The parser promotes inline column constraints, PRIMARY KEY included,
    // to `ct.constraints`, so a column's `is_inline_pk` adds no constraint.
    let named_constraints = create_table_constraint_names(
        ct.name.name(),
        &ct.constraints,
        catalog.names_in_schema(&table_key.schema),
    );

    // `insert_table` registers every index the new table ends up with, so
    // these updates need not be applied.
    let mut index_registry = IndexRegistryUpdates::default();
    for (constraint, name) in named_constraints {
        apply_table_constraint(&mut table, constraint, name, &mut index_registry);
    }

    catalog.insert_table(table);
    warning
}

/// Handle ALTER TABLE: apply each action to the existing table.
/// If the table does not exist in the catalog, silently skip.
///
/// Postgres runs the subcommands in passes rather than in textual order, and
/// replay follows the passes that change what a statement leaves behind:
/// drops first, then column changes, then the added constraints, then
/// VALIDATE CONSTRAINT. A name dropped anywhere in the statement is free for
/// a constraint it adds (`verify/tests/naming/collision_order.sql`).
fn apply_alter_table(catalog: &mut Catalog, at: &AlterTable) {
    let table_key = at.name.relation();

    // If the table doesn't exist, silently skip. It may be in a different
    // schema or managed outside our tracked migrations.
    if !catalog.has_table(table_key) {
        return;
    }

    update_table(catalog, table_key, |table, index_registry| {
        for action in &at.actions {
            match action {
                AlterTableAction::DropColumn { name } => {
                    drop_column(table, name, index_registry);
                }
                AlterTableAction::DropConstraint { constraint_name } => {
                    drop_constraint(table, constraint_name, index_registry);
                }
                _ => {}
            }
        }
    });

    update_table(catalog, table_key, |table, _| {
        for action in &at.actions {
            apply_column_change(table, action);
        }
    });

    // An `ADD COLUMN`'s inline constraints, PRIMARY KEY included, arrive as
    // `AddConstraint` actions of their own, with origin `AddColumn`.
    let named_constraints = alter_table_constraint_names(
        at.name.name(),
        &at.actions,
        catalog.names_in_schema(&table_key.schema),
    );
    update_table(catalog, table_key, |table, index_registry| {
        for (constraint, name) in named_constraints {
            apply_table_constraint(table, constraint, name, index_registry);
        }
    });

    update_table(catalog, table_key, |table, _| {
        for action in &at.actions {
            if let AlterTableAction::ValidateConstraint { constraint_name } = action {
                validate_constraint(table, constraint_name);
            }
        }
    });

    for action in &at.actions {
        match action {
            AlterTableAction::AttachPartition { child } => {
                if let Some(child_table) = catalog.get_table_mut(child.relation()) {
                    child_table.parent_table = Some(table_key.clone());
                }
            }
            AlterTableAction::DetachPartition { child, .. } => {
                if let Some(child_table) = catalog.get_table_mut(child.relation()) {
                    child_table.parent_table = None;
                }
            }
            _ => {}
        }
    }
}

/// Apply `update` to the table `table_key`, then the index registry changes
/// it collected. The table stays mutably borrowed for the whole update, so
/// the registry, which lives in the catalog, can only change afterwards.
fn update_table(
    catalog: &mut Catalog,
    table_key: &RelationName,
    update: impl FnOnce(&mut TableState, &mut IndexRegistryUpdates),
) {
    let Some(table) = catalog.get_table_mut(table_key) else {
        return;
    };
    let mut index_registry = IndexRegistryUpdates::default();
    update(table, &mut index_registry);
    index_registry.apply(catalog, table_key);
}

/// Apply an ALTER TABLE action that changes the table's columns. Drops,
/// added constraints, VALIDATE CONSTRAINT and partitions have passes of
/// their own in [`apply_alter_table`].
fn apply_column_change(table: &mut TableState, action: &AlterTableAction) {
    match action {
        AlterTableAction::AddColumn(col_def) => {
            table.columns.push(column_def_to_state(col_def));
        }
        AlterTableAction::AlterColumnType {
            column_name,
            new_type,
            ..
        } => {
            if let Some(col) = table.get_column_mut(column_name) {
                col.type_name = new_type.clone();
            }
        }
        AlterTableAction::SetNotNull { column_name } => {
            if let Some(col) = table.get_column_mut(column_name) {
                col.nullable = false;
            }
        }
        AlterTableAction::DropNotNull { column_name } => {
            if let Some(col) = table.get_column_mut(column_name) {
                col.nullable = true;
            }
        }
        AlterTableAction::SetDefault {
            column_name,
            default_expr,
        } => {
            if let Some(col) = table.get_column_mut(column_name) {
                col.has_default = true;
                col.default_expr = Some(default_expr.clone());
            }
        }
        AlterTableAction::DropDefault { column_name } => {
            if let Some(col) = table.get_column_mut(column_name) {
                col.has_default = false;
                col.default_expr = None;
            }
        }
        AlterTableAction::DropColumn { .. }
        | AlterTableAction::DropConstraint { .. }
        | AlterTableAction::AddConstraint { .. }
        | AlterTableAction::ValidateConstraint { .. }
        | AlterTableAction::AttachPartition { .. }
        | AlterTableAction::DetachPartition { .. } => {}
        AlterTableAction::DisableTrigger { .. } => { /* triggers not tracked */ }
        AlterTableAction::Other { .. } => { /* ignore unmodeled actions */ }
    }
}

/// Handle `ALTER TABLE ... DROP COLUMN`: remove the column, and with it every
/// index and constraint that references it. Expression indexes count too
/// (`lower(email)` references `email`).
fn drop_column(table: &mut TableState, name: &str, index_registry: &mut IndexRegistryUpdates) {
    for idx in &table.indexes {
        if idx.references_column(name) {
            index_registry.unregister.push(idx.name.clone());
        }
    }
    table.remove_column(name);
}

/// Handle `ALTER TABLE ... VALIDATE CONSTRAINT`: a NOT VALID foreign key or
/// CHECK of that name becomes valid.
fn validate_constraint(table: &mut TableState, constraint_name: &str) {
    for c in &mut table.constraints {
        match c {
            ConstraintState::ForeignKey {
                name, not_valid, ..
            }
            | ConstraintState::Check {
                name, not_valid, ..
            } if name == constraint_name => {
                *not_valid = false;
            }
            _ => {}
        }
    }
}

/// Handle CREATE INDEX: add an index to the target table.
/// If the table does not exist in the catalog, silently skip.
///
/// When `IF NOT EXISTS` is used and a relation of that name already exists in
/// the table's schema, PostgreSQL skips the statement with a notice (see
/// [`if_not_exists_skips`]). We keep the catalog as it is and return a
/// warning, as for replacing an existing index.
fn apply_create_index(catalog: &mut Catalog, ci: &CreateIndex) -> Option<String> {
    let table_key = ci.table_name.relation();

    // A generated name never clashes: it gets a collision suffix instead.
    // Only an explicit name can already be taken.
    let index_name = created_index_name(catalog, ci);
    // The grammar allows no schema on the index name: the index is created
    // in its table's schema.
    let index_key = table_key.in_same_schema(&index_name);

    if if_not_exists_skips(catalog, ci) {
        return Some(format!(
            "CREATE INDEX IF NOT EXISTS `{}` skipped: a relation of that name already exists \
             in catalog. The migration chain may be inconsistent.",
            index_name
        ));
    }

    let mut warning = None;
    if ci.index_name.is_some() && catalog.get_index(&index_key).is_some() {
        warning = Some(format!(
            "CREATE INDEX `{}` overwrites existing index in catalog. \
             The index may have been dropped outside tracked migrations, or this is a duplicate definition.",
            index_name
        ));
    }

    let Some(table) = catalog.get_table_mut(table_key) else {
        return warning;
    };

    let entries: Vec<IndexColumn> = ci.columns.to_vec();

    table.indexes.push(IndexState {
        name: index_name.clone(),
        entries,
        unique: ci.unique,
        where_clause: ci.where_clause.clone(),
        only: ci.only,
        access_method: ci.access_method.clone(),
    });

    // Register after confirming the table exists, to avoid ghost entries.
    catalog.register_index(&index_key, table_key);
    warning
}

/// The name of the index `ci` creates on top of `catalog`: its own name, or
/// the generated `{table}_{elements}_idx` with the first collision suffix
/// that no relation in the table's schema has.
fn created_index_name(catalog: &Catalog, ci: &CreateIndex) -> String {
    match &ci.index_name {
        Some(name) => name.clone(),
        None => GeneratedName::index(ci.table_name.name(), &ci.columns, &ci.include_columns)
            .choose(&catalog.names_in_schema(&ci.table_name.relation().schema)),
    }
}

/// True if `ci` is a `CREATE INDEX IF NOT EXISTS` that PostgreSQL skips on
/// top of `catalog`, creating nothing: a relation in the table's schema
/// already has its name. Any relation counts, a table as much as an index
/// (`index_create` looks the name up in `pg_class`). The grammar requires a
/// name with `IF NOT EXISTS`.
fn if_not_exists_skips(catalog: &Catalog, ci: &CreateIndex) -> bool {
    ci.if_not_exists
        && ci.index_name.as_deref().is_some_and(|name| {
            catalog
                .names_in_schema(&ci.table_name.relation().schema)
                .relation_taken(name)
        })
}

/// For each of `statements`, the name of the index it creates, as replaying
/// them on top of `catalog` names it. `None` when it creates none: it is not
/// a `CREATE INDEX`, or it is a `CREATE INDEX IF NOT EXISTS` that PostgreSQL
/// skips because its name is taken ([`if_not_exists_skips`]). A generated
/// name, and whether a name is taken, depend on every relation created or
/// dropped before the statement, in the schema and earlier in `statements`,
/// so each is judged against the state [`map_with_state_before_if_needed`]
/// gives it, which replays the earlier statements only when some statement
/// asks either question ([`created_index_name_depends_on_state`]).
pub(crate) fn created_index_names(
    catalog: &Catalog,
    statements: &[Located<IrNode>],
) -> Vec<Option<String>> {
    map_with_state_before_if_needed(
        catalog,
        statements,
        created_index_name_depends_on_state,
        |state, stmt| match &stmt.node {
            IrNode::CreateIndex(ci) if !if_not_exists_skips(state, ci) => {
                Some(created_index_name(state, ci))
            }
            _ => None,
        },
    )
}

/// True for a `CREATE INDEX` whose created index name depends on the
/// relations before it: one that generates its name, or an `IF NOT EXISTS`
/// one, which creates nothing when its name is taken. An explicitly named
/// `CREATE INDEX` without `IF NOT EXISTS` creates the index of that name
/// whatever the catalog holds.
fn created_index_name_depends_on_state(node: &IrNode) -> bool {
    matches!(node, IrNode::CreateIndex(ci) if ci.index_name.is_none() || ci.if_not_exists)
}

/// Call `visit` on each of `statements` with the catalog just before it:
/// `catalog` with the statements before it replayed on top, as the
/// pipeline's replay of the unit leaves it at that point. The replay runs on
/// a copy of `catalog`, and its warnings are dropped: the pipeline's own
/// replay of the same statements reports them.
pub(crate) fn map_with_state_before<'s, T>(
    catalog: &Catalog,
    statements: &'s [Located<IrNode>],
    mut visit: impl FnMut(&Catalog, &'s Located<IrNode>) -> T,
) -> Vec<T> {
    let mut replayed = catalog.clone();
    statements
        .iter()
        .map(|stmt| {
            let visited = visit(&replayed, stmt);
            let _warning = apply_node(&mut replayed, &stmt.node);
            visited
        })
        .collect()
}

/// [`map_with_state_before`] for a `visit` that reads the catalog only for
/// the statements `needs_state` holds for. A unit without such a statement
/// needs no replay, so `visit` then gets `catalog` itself for every
/// statement.
pub(crate) fn map_with_state_before_if_needed<'s, T>(
    catalog: &Catalog,
    statements: &'s [Located<IrNode>],
    needs_state: impl Fn(&IrNode) -> bool,
    mut visit: impl FnMut(&Catalog, &'s Located<IrNode>) -> T,
) -> Vec<T> {
    if statements.iter().any(|stmt| needs_state(&stmt.node)) {
        return map_with_state_before(catalog, statements, visit);
    }
    statements.iter().map(|stmt| visit(catalog, stmt)).collect()
}

/// Handle DROP INDEX: remove the index from the table that owns it, looked
/// up in the index's resolved schema only. If there is no such index,
/// silently skip. Postgres rejects dropping the index of a PRIMARY KEY,
/// UNIQUE or EXCLUDE constraint (2BP01, [`TableState::constraint_backed_by`])
/// and changes nothing, so that is skipped too.
fn apply_drop_index(catalog: &mut Catalog, di: &DropIndex) {
    let index_key = di.index_name.relation();
    let Some(table_name) = catalog.table_for_index(index_key).cloned() else {
        return;
    };
    let required_by_a_constraint = catalog
        .get_table(&table_name)
        .is_some_and(|table| table.constraint_backed_by(&index_key.name).is_some());
    if required_by_a_constraint {
        return;
    }

    catalog.unregister_index(index_key, &table_name);

    if let Some(table) = catalog.get_table_mut(&table_name) {
        table.indexes.retain(|idx| idx.name != index_key.name);
    }
}

/// Handle `ALTER INDEX ... RENAME TO`: the index keeps its schema and table
/// and takes the bare `new_name`.
///
/// Postgres renames the constraint an index backs along with the index
/// (`verify/tests/naming/rename.sql`: `ALTER INDEX rename_idx_pkey RENAME TO
/// rename_idx_new_pk` leaves the PRIMARY KEY named `rename_idx_new_pk`), so a
/// PRIMARY KEY, UNIQUE or EXCLUDE constraint of the old name follows. CHECK
/// and FOREIGN KEY constraints are never index-backed and keep their names.
/// A missing index is skipped, with or without `IF EXISTS`, and so is a
/// rename Postgres rejects because the new name is taken
/// ([`Catalog::index_rename_clash`]).
fn apply_rename_index(catalog: &mut Catalog, index: &ResolvedName, new_name: &str) {
    let old_key = index.relation();
    let Some(table_key) = catalog.table_for_index(old_key).cloned() else {
        return;
    };
    if catalog.index_rename_clash(old_key, new_name).is_some() {
        return;
    }
    rename_index_and_its_constraint(catalog, &table_key, &old_key.name, new_name);
}

/// Handle `ALTER TABLE ... RENAME CONSTRAINT`. Renaming a PRIMARY KEY,
/// UNIQUE or EXCLUDE constraint renames its index too
/// (`verify/tests/naming/rename.sql`), as `ALTER INDEX ... RENAME` does the
/// other way round. A CHECK or FOREIGN KEY constraint renames alone. A
/// missing table or constraint is skipped, with or without `IF EXISTS`, and
/// so is a rename Postgres rejects because the new name is taken
/// ([`Catalog::constraint_rename_clash`]).
fn apply_rename_constraint(
    catalog: &mut Catalog,
    table: &ResolvedName,
    old_name: &str,
    new_name: &str,
) {
    let table_key = table.relation();
    let Some(is_index_backed) = catalog
        .get_table(table_key)
        .and_then(|t| t.get_constraint(old_name))
        .map(ConstraintState::is_index_backed)
    else {
        return;
    };
    if catalog
        .constraint_rename_clash(table_key, old_name, new_name)
        .is_some()
    {
        return;
    }

    if is_index_backed {
        rename_index_and_its_constraint(catalog, table_key, old_name, new_name);
    } else {
        update_table(catalog, table_key, |table, _| {
            if let Some(constraint) = table.constraints.iter_mut().find(|c| c.name() == old_name) {
                constraint.rename(new_name);
            }
        });
    }
}

/// Rename the index `old_name` of `table_key`, and the PRIMARY KEY, UNIQUE
/// or EXCLUDE constraint of that name, to `new_name`: Postgres keeps an
/// index-backed constraint and its index under one name whichever of the
/// two is renamed. The catalog records no index for an EXCLUDE constraint,
/// so only the constraint is renamed and no index is registered under the
/// new name; [`Catalog::names_in_schema`] still counts it as a relation.
fn rename_index_and_its_constraint(
    catalog: &mut Catalog,
    table_key: &RelationName,
    old_name: &str,
    new_name: &str,
) {
    update_table(catalog, table_key, |table, index_registry| {
        for idx in table.indexes.iter_mut().filter(|idx| idx.name == old_name) {
            idx.name = new_name.to_string();
            index_registry.rename(old_name, new_name);
        }
        for constraint in table
            .constraints
            .iter_mut()
            .filter(|c| c.is_index_backed() && c.name() == old_name)
        {
            constraint.rename(new_name);
        }
    });
}

/// Handle ALTER INDEX ... ATTACH PARTITION: flip `only` to `false` on the parent index.
///
/// When all child partitions have their own indexes attached, the parent
/// ON ONLY index becomes a valid recursive index covering all partitions.
fn apply_alter_index_attach(catalog: &mut Catalog, parent_index: &RelationName) {
    let Some(table_key) = catalog.table_for_index(parent_index).cloned() else {
        return;
    };
    let Some(table) = catalog.get_table_mut(&table_key) else {
        return;
    };
    if let Some(idx) = table
        .indexes
        .iter_mut()
        .find(|i| i.name == parent_index.name)
    {
        idx.only = false;
    }
}

/// Handle DROP TABLE: remove the table from the catalog entirely.
///
/// Dropping a partitioned parent always removes its partition subtree
/// (depth-first), regardless of CASCADE: partitions aren't independent
/// dependent objects the way a view or FK is, they're structurally part of
/// the parent, so PostgreSQL drops them whether or not CASCADE is given.
/// CASCADE still governs other dependent objects (views, FK constraints)
/// that this catalog does not track.
fn apply_drop_table(catalog: &mut Catalog, dt: &DropTable) {
    let table_key = dt.name.relation();

    // Only recurse into the partition subtree if the table actually exists in
    // the catalog. This guards against `DROP TABLE <name-not-in-catalog>`
    // accidentally sweeping up unrelated tables whose `parent_table` happens
    // to equal that same string (see e.g. a child created via
    // `PARTITION OF unknown_parent` before `unknown_parent` was ever created).
    if catalog.has_table(table_key) {
        let children_to_remove = collect_partition_subtree(catalog, table_key);
        for child_key in children_to_remove {
            catalog.remove_table(&child_key);
        }
    }

    catalog.remove_table(table_key);
}

/// Handle DROP SCHEMA: remove all tables in the schema from the catalog.
///
/// With CASCADE, all tables in that schema are removed. Without CASCADE,
/// PostgreSQL would error at runtime if the schema is non-empty, so we treat
/// it as a no-op.
fn apply_drop_schema(catalog: &mut Catalog, ds: &DropSchema) {
    if !ds.cascade {
        return;
    }

    let keys_to_remove: Vec<RelationName> = catalog
        .tables()
        .filter(|t| t.name.schema == ds.schema_name)
        .map(|t| t.name.clone())
        .collect();

    for key in keys_to_remove {
        catalog.remove_table(&key);
    }
}

/// Collect all partition children recursively (depth-first), for removal when
/// dropping a partitioned parent. Returns an empty `Vec` for a table with no
/// children, so calling this unconditionally costs nothing for non-partitioned
/// tables. Uses a visited set to prevent cycles.
fn collect_partition_subtree(catalog: &Catalog, root_key: &RelationName) -> Vec<RelationName> {
    let mut result = Vec::new();
    let mut stack = vec![root_key.clone()];
    let mut visited = std::collections::HashSet::new();

    while let Some(key) = stack.pop() {
        if !visited.insert(key.clone()) {
            continue;
        }
        for child in catalog.get_partition_children(&key) {
            result.push(child.clone());
            stack.push(child);
        }
    }

    result
}

/// Handle RENAME TABLE: move the table state to a new key in the same schema.
///
/// Also updates partition tracking:
/// - If the renamed table is a partitioned parent: updates each child's
///   `parent_table` to point to the new key.
/// - If the renamed table is a partition child: `parent_table` is preserved
///   automatically (carried over on the `TableState`).
///
/// A rename Postgres rejects because the new name is taken
/// ([`Catalog::table_rename_clash`]) is skipped.
fn apply_rename_table(catalog: &mut Catalog, name: &ResolvedName, new_name: &str) {
    let old_key = name.relation();
    if catalog.table_rename_clash(old_key, new_name).is_some() {
        return;
    }

    // Compute children before remove_table removes the table from the HashMap.
    let children = catalog.get_partition_children(old_key);

    if let Some(mut table) = catalog.remove_table(old_key) {
        let new_key = old_key.in_same_schema(new_name);

        // Update each child's parent_table to point to the new key.
        for child_key in &children {
            if let Some(child) = catalog.get_table_mut(child_key) {
                child.parent_table = Some(new_key.clone());
            }
        }

        table.name = new_key.clone();
        table.display_name = new_name.to_string();
        catalog.insert_table(table);
    }
}

/// Handle RENAME COLUMN: rename a column in a table, updating indexes and constraints.
fn apply_rename_column(
    catalog: &mut Catalog,
    table_name: &ResolvedName,
    old_name: &str,
    new_name: &str,
) {
    let Some(table) = catalog.get_table_mut(table_name.relation()) else {
        return;
    };

    // Rename the column itself.
    if let Some(col) = table.get_column_mut(old_name) {
        col.name = new_name.to_string();
    }

    // Update partition key columns if this table is partitioned.
    if let Some(ref mut pb) = table.partition_by {
        for col in &mut pb.columns {
            if *col == old_name {
                *col = new_name.to_string();
            }
        }
    }

    // Update indexes that reference the old column name.
    // Both plain column entries and expression referenced_columns are updated.
    // Expression *text* is left as-is (it becomes stale after rename, but nothing
    // in the codebase matches on expression text content).
    for idx in &mut table.indexes {
        rename_in_index_columns(&mut idx.entries, old_name, new_name);
    }

    // Update constraints that reference the old column name.
    let table_name_key = table.name.clone();
    for constraint in &mut table.constraints {
        match constraint {
            ConstraintState::PrimaryKey { columns, .. }
            | ConstraintState::Unique { columns, .. } => {
                for col in columns {
                    if *col == old_name {
                        *col = new_name.to_string();
                    }
                }
            }
            ConstraintState::ForeignKey {
                columns,
                ref_table,
                ref_columns,
                ..
            } => {
                for col in columns.iter_mut() {
                    if *col == old_name {
                        *col = new_name.to_string();
                    }
                }
                // For self-referencing FKs, also rename matching ref_columns.
                if *ref_table == table_name_key {
                    for col in ref_columns.iter_mut() {
                        if *col == old_name {
                            *col = new_name.to_string();
                        }
                    }
                }
            }
            ConstraintState::Check { expression, .. } => {
                *expression = replace_column_in_expression(expression, old_name, new_name);
            }
            ConstraintState::Exclude { elements, .. } => {
                rename_in_index_columns(elements, old_name, new_name);
            }
        }
    }
}

/// Rename a column within a list of `IndexColumn` entries, in place.
///
/// Handles both plain column entries and expression `referenced_columns`.
/// Shared by index entries and EXCLUDE constraint elements, which use the
/// same `IndexColumn` representation. Expression *text* is left as-is (it
/// becomes stale after rename, but nothing in the codebase matches on
/// expression text content).
fn rename_in_index_columns(entries: &mut [IndexColumn], old_name: &str, new_name: &str) {
    for entry in entries {
        match entry {
            IndexColumn::Column(col) => {
                if col == old_name {
                    *col = new_name.to_string();
                }
            }
            IndexColumn::Expression {
                referenced_columns, ..
            } => {
                for col in referenced_columns {
                    if col == old_name {
                        *col = new_name.to_string();
                    }
                }
            }
        }
    }
}

/// Replace a column name in an expression string, respecting word boundaries.
///
/// Splits the expression into identifier tokens (alphanumeric + underscore)
/// and non-identifier separators. Tokens matching `old` are replaced with `new`.
/// This avoids replacing substrings (e.g., renaming `id` won't affect `id_type`).
fn replace_column_in_expression(expression: &str, old: &str, new: &str) -> String {
    let mut result = String::with_capacity(expression.len());
    let mut token = String::new();

    for ch in expression.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            token.push(ch);
        } else {
            if !token.is_empty() {
                if token == old {
                    result.push_str(new);
                } else {
                    result.push_str(&token);
                }
                token.clear();
            }
            result.push(ch);
        }
    }
    // Flush remaining token
    if !token.is_empty() {
        if token == old {
            result.push_str(new);
        } else {
            result.push_str(&token);
        }
    }
    result
}

/// Handle Unparseable: if a table_hint is provided, mark that table as incomplete.
fn apply_unparseable(catalog: &mut Catalog, table_hint: Option<&ResolvedName>) {
    if let Some(hint) = table_hint
        && let Some(table) = catalog.get_table_mut(hint.relation())
    {
        table.incomplete = true;
    }
}

/// Convert an IR ColumnDef to a catalog ColumnState.
fn column_def_to_state(col: &ColumnDef) -> ColumnState {
    ColumnState {
        name: col.name.clone(),
        type_name: col.type_name.clone(),
        nullable: col.nullable,
        has_default: col.default_expr.is_some(),
        default_expr: col.default_expr.clone(),
    }
}

/// Index-name reverse-map changes collected while a table is mutably
/// borrowed, applied to the catalog once the borrow ends. The names are the
/// bare names of the table's own indexes, which live in the table's schema.
#[derive(Default)]
struct IndexRegistryUpdates {
    register: Vec<String>,
    unregister: Vec<String>,
}

impl IndexRegistryUpdates {
    fn rename(&mut self, old_name: &str, new_name: &str) {
        self.unregister.push(old_name.to_string());
        self.register.push(new_name.to_string());
    }

    fn apply(self, catalog: &mut Catalog, table_key: &RelationName) {
        for name in self.unregister {
            catalog.unregister_index(&table_key.in_same_schema(name), table_key);
        }
        for name in self.register {
            catalog.register_index(&table_key.in_same_schema(name), table_key);
        }
    }
}

/// Handle `ALTER TABLE ... DROP CONSTRAINT`: remove the constraint with that
/// name, and the index of the same name when the constraint is index-backed.
fn drop_constraint(
    table: &mut TableState,
    constraint_name: &str,
    index_registry: &mut IndexRegistryUpdates,
) {
    let (dropped, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut table.constraints)
        .into_iter()
        .partition(|c| c.name() == constraint_name);
    table.constraints = kept;
    table.has_primary_key = table
        .constraints
        .iter()
        .any(|c| matches!(c, ConstraintState::PrimaryKey { .. }));

    for constraint in dropped.iter().filter(|c| c.is_index_backed()) {
        let index_name = constraint.name();
        if table.indexes.iter().any(|idx| idx.name == index_name) {
            table.indexes.retain(|idx| idx.name != index_name);
            index_registry.unregister.push(index_name.to_string());
        }
    }
}

/// Give a PRIMARY KEY or UNIQUE constraint its backing index, which always
/// carries the constraint's name, and return the constraint's key columns.
///
/// Without `USING INDEX`, a new unique btree index is created. With
/// `USING INDEX idx` the existing index becomes the backing index: an
/// unnamed constraint takes the name `idx`, and a named one renames the
/// index to its own name. The IR has no columns in that case, so they come
/// from the index. Expression entries are left out: PostgreSQL rejects an
/// expression index there.
fn attach_backing_index(
    table: &mut TableState,
    constraint_name: &str,
    columns: &[String],
    using_index: Option<&str>,
    index_registry: &mut IndexRegistryUpdates,
) -> Vec<String> {
    let Some(using_index) = using_index else {
        table.indexes.push(IndexState {
            name: constraint_name.to_string(),
            entries: columns
                .iter()
                .map(|c| IndexColumn::Column(c.clone()))
                .collect(),
            unique: true,
            where_clause: None,
            only: false,
            access_method: IndexState::DEFAULT_ACCESS_METHOD.to_string(),
        });
        index_registry.register.push(constraint_name.to_string());
        return columns.to_vec();
    };

    let Some(index) = table.indexes.iter_mut().find(|idx| idx.name == using_index) else {
        return Vec::new();
    };
    if index.name != constraint_name {
        index.name = constraint_name.to_string();
        index_registry.rename(using_index, constraint_name);
    }
    index.column_names().map(str::to_string).collect()
}

/// Apply a table constraint, named `name`, to the table state, updating the
/// constraints list and has_primary_key flag as needed.
fn apply_table_constraint(
    table: &mut TableState,
    constraint: &TableConstraint,
    name: String,
    index_registry: &mut IndexRegistryUpdates,
) {
    match constraint {
        TableConstraint::PrimaryKey {
            columns,
            using_index,
            ..
        } => {
            table.has_primary_key = true;
            let columns = attach_backing_index(
                table,
                &name,
                columns,
                using_index.as_deref(),
                index_registry,
            );
            table
                .constraints
                .push(ConstraintState::PrimaryKey { name, columns });
        }
        TableConstraint::ForeignKey {
            columns,
            ref_table,
            ref_columns,
            not_valid,
            ..
        } => {
            table.constraints.push(ConstraintState::ForeignKey {
                name,
                columns: columns.clone(),
                ref_table: ref_table.relation().clone(),
                ref_table_display: ref_table.display_name(),
                ref_columns: ref_columns.clone(),
                not_valid: *not_valid,
            });
        }
        TableConstraint::Unique {
            columns,
            using_index,
            ..
        } => {
            let columns = attach_backing_index(
                table,
                &name,
                columns,
                using_index.as_deref(),
                index_registry,
            );
            table
                .constraints
                .push(ConstraintState::Unique { name, columns });
        }
        TableConstraint::Check {
            expression,
            not_valid,
            ..
        } => {
            table.constraints.push(ConstraintState::Check {
                name,
                expression: expression.clone(),
                not_valid: *not_valid,
            });
        }
        TableConstraint::Exclude { elements, .. } => {
            table.constraints.push(ConstraintState::Exclude {
                name,
                elements: elements.clone(),
            });
        }
    }
}
