//! Catalog replay engine — applies IR nodes to mutate the catalog.
//!
//! The replay engine processes migration units in order, applying each
//! IR statement to build up the table catalog. This is the core of the
//! single-pass replay strategy: the pipeline calls [`apply`] for each
//! migration unit, and the catalog accumulates state over time.

use crate::catalog::generated_names::{GeneratedName, created_index_name};
use crate::catalog::types::*;
use crate::input::MigrationUnit;
use crate::names::{RelationName, ResolvedName};
use crate::parser::ir::*;

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
        apply_node(catalog, &located.node);
    }
}

/// Apply a single IR node to the catalog.
fn apply_node(catalog: &mut Catalog, node: &IrNode) {
    match node {
        IrNode::CreateTable(ct) => apply_create_table(catalog, ct),
        IrNode::AlterTable(at) => apply_alter_table(catalog, at),
        IrNode::CreateIndex(ci) => apply_create_index(catalog, ci),
        IrNode::DropIndex(di) => apply_drop_index(catalog, di),
        IrNode::DropTable(dt) => apply_drop_table(catalog, dt),
        IrNode::RenameTable { name, new_name } => apply_rename_table(catalog, name, new_name),
        IrNode::RenameColumn {
            table,
            old_name,
            new_name,
        } => apply_rename_column(catalog, table, old_name, new_name),
        IrNode::RenameIndex { .. } => todo!("we'll fix this before release. promise"),
        IrNode::AlterIndexAttachPartition {
            parent_index_name, ..
        } => apply_alter_index_attach(catalog, parent_index_name.name()),
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
}

/// Handle CREATE TABLE: insert a new table into the catalog with columns,
/// constraints, and indexes derived from the statement.
///
/// When `IF NOT EXISTS` is used and the table already exists, the statement
/// is a no-op in PostgreSQL. We mirror that by keeping the existing catalog
/// state and emitting a warning — the migration chain is ambiguous at that
/// point (which definition is the truth?).
fn apply_create_table(catalog: &mut Catalog, ct: &CreateTable) {
    let table_key = ct.name.relation().clone();

    if catalog.has_table(&table_key) {
        if ct.if_not_exists {
            eprintln!(
                "warning: CREATE TABLE IF NOT EXISTS `{}` skipped — table already exists in catalog. \
                 The migration chain may be inconsistent.",
                ct.name.display_name()
            );
            return;
        }
        eprintln!(
            "warning: CREATE TABLE `{}` overwrites existing table in catalog. \
             The table may have been dropped outside tracked migrations, or this is a duplicate definition.",
            ct.name.display_name()
        );
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

    // `insert_table` registers every index the new table ends up with, so
    // these updates need not be applied.
    let mut index_registry = IndexRegistryUpdates::default();

    // Convert columns (explicit columns on the child, or regular table columns)
    for col in &ct.columns {
        table.columns.push(column_def_to_state(col));

        // Handle inline PK on the column definition
        if col.is_inline_pk {
            apply_table_constraint(
                &mut table,
                ct.name.name(),
                &TableConstraint::PrimaryKey {
                    name: None,
                    columns: vec![col.name.clone()],
                    include_columns: vec![],
                    using_index: None,
                },
                &mut index_registry,
            );
        }
    }

    // Convert table-level constraints
    for constraint in &ct.constraints {
        apply_table_constraint(&mut table, ct.name.name(), constraint, &mut index_registry);
    }

    catalog.insert_table(table);
}

/// Handle ALTER TABLE: apply each action to the existing table.
/// If the table does not exist in the catalog, silently skip.
fn apply_alter_table(catalog: &mut Catalog, at: &AlterTable) {
    let table_key = at.name.relation();
    let bare_table_name = at.name.name();

    // If the table doesn't exist, silently skip. It may be in a different
    // schema or managed outside our tracked migrations.
    if !catalog.has_table(table_key) {
        return;
    }

    // Collect indexes to register/unregister so we can update the reverse map
    // after releasing the mutable borrow on the table.
    let mut index_registry = IndexRegistryUpdates::default();

    {
        let Some(table) = catalog.get_table_mut(table_key) else {
            return;
        };

        for action in &at.actions {
            match action {
                AlterTableAction::AddColumn(col_def) => {
                    table.columns.push(column_def_to_state(col_def));

                    // Handle inline PK on the added column
                    if col_def.is_inline_pk {
                        apply_table_constraint(
                            table,
                            bare_table_name,
                            &TableConstraint::PrimaryKey {
                                name: None,
                                columns: vec![col_def.name.clone()],
                                include_columns: vec![],
                                using_index: None,
                            },
                            &mut index_registry,
                        );
                    }
                }
                AlterTableAction::DropColumn { name } => {
                    // Collect index names that will be removed by the column drop.
                    // Uses references_column() to also detect expression indexes
                    // that reference the dropped column (e.g. `lower(email)`).
                    for idx in &table.indexes {
                        if idx.references_column(name) {
                            index_registry.unregister.push(idx.name.clone());
                        }
                    }
                    table.remove_column(name);
                }
                AlterTableAction::AddConstraint(constraint) => {
                    apply_table_constraint(table, bare_table_name, constraint, &mut index_registry);
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
                AlterTableAction::DropConstraint { constraint_name } => {
                    drop_constraint(table, constraint_name, &mut index_registry);
                }
                AlterTableAction::ValidateConstraint { constraint_name } => {
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
                AlterTableAction::AttachPartition { .. }
                | AlterTableAction::DetachPartition { .. } => {
                    // Handled below, outside the table mutable borrow.
                }
                AlterTableAction::DisableTrigger { .. } => { /* triggers not tracked */ }
                AlterTableAction::Other { .. } => { /* ignore unmodeled actions */ }
            }
        }
    }

    // Update reverse map outside the table borrow.
    index_registry.apply(catalog, table_key);

    // Handle partition attach/detach outside the table borrow scope.
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

/// Handle CREATE INDEX: add an index to the target table.
/// If the table does not exist in the catalog, silently skip.
///
/// When `IF NOT EXISTS` is used and a same-named index already exists,
/// PostgreSQL treats it as a no-op. We keep the existing index and warn.
fn apply_create_index(catalog: &mut Catalog, ci: &CreateIndex) {
    let table_key = ci.table_name.relation();

    // Postgres never lets a generated name collide: it appends a number
    // instead. Collision suffixes are not modelled yet, so only an explicit
    // name can meaningfully clash with an existing index.
    let index_name = created_index_name(ci);

    if ci.index_name.is_some() && catalog.get_index(&index_name).is_some() {
        if ci.if_not_exists {
            eprintln!(
                "warning: CREATE INDEX IF NOT EXISTS `{}` skipped — index already exists in catalog. \
                 The migration chain may be inconsistent.",
                index_name
            );
            return;
        }
        eprintln!(
            "warning: CREATE INDEX `{}` overwrites existing index in catalog. \
             The index may have been dropped outside tracked migrations, or this is a duplicate definition.",
            index_name
        );
    }

    let Some(table) = catalog.get_table_mut(table_key) else {
        return;
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
    catalog.register_index(&index_name, table_key);
}

/// Handle DROP INDEX: find and remove the named index from whichever table has it.
/// If no table has the index, silently skip.
fn apply_drop_index(catalog: &mut Catalog, di: &DropIndex) {
    let index_name = di.index_name.name();
    let Some(table_name) = catalog.table_for_index(index_name).cloned() else {
        return;
    };

    catalog.unregister_index(index_name);

    if let Some(table) = catalog.get_table_mut(&table_name) {
        table.indexes.retain(|idx| idx.name != index_name);
    }
}

/// Handle ALTER INDEX ... ATTACH PARTITION: flip `only` to `false` on the parent index.
///
/// When all child partitions have their own indexes attached, the parent
/// ON ONLY index becomes a valid recursive index covering all partitions.
fn apply_alter_index_attach(catalog: &mut Catalog, parent_index_name: &str) {
    let Some(table_key) = catalog.table_for_index(parent_index_name).cloned() else {
        return;
    };
    let Some(table) = catalog.get_table_mut(&table_key) else {
        return;
    };
    if let Some(idx) = table
        .indexes
        .iter_mut()
        .find(|i| i.name == parent_index_name)
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
fn apply_rename_table(catalog: &mut Catalog, name: &ResolvedName, new_name: &str) {
    let old_key = name.relation();

    // Compute children before remove_table removes the table from the HashMap.
    let children = catalog.get_partition_children(old_key);

    if let Some(mut table) = catalog.remove_table(old_key) {
        let new_key = RelationName::new(old_key.schema.clone(), new_name);

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
/// borrowed, applied to the catalog once the borrow ends.
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
            catalog.unregister_index(&name);
        }
        for name in self.register {
            catalog.register_index(&name, table_key);
        }
    }
}

/// Handle `ALTER TABLE ... DROP CONSTRAINT`: remove the constraint with that
/// name, and the index of the same name when the constraint is index-backed.
///
/// A table has at most one primary key, so dropping it removes every
/// PRIMARY KEY entry; an inline PK can be registered twice (see TODO.md).
fn drop_constraint(
    table: &mut TableState,
    constraint_name: &str,
    index_registry: &mut IndexRegistryUpdates,
) {
    let dropping_pk = table
        .constraints
        .iter()
        .any(|c| matches!(c, ConstraintState::PrimaryKey { .. }) && c.name() == constraint_name);
    let (dropped, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut table.constraints)
        .into_iter()
        .partition(|c| {
            c.name() == constraint_name
                || (dropping_pk && matches!(c, ConstraintState::PrimaryKey { .. }))
        });
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

/// The name of an index-backed PRIMARY KEY or UNIQUE constraint: the given
/// name, else the `USING INDEX` index's name, else the generated one.
fn index_backed_constraint_name(
    name: &Option<String>,
    using_index: &Option<String>,
    generate: impl FnOnce() -> GeneratedName,
) -> String {
    name.clone()
        .or_else(|| using_index.clone())
        .unwrap_or_else(|| generate().unsuffixed())
}

/// Apply a table constraint to the table state, updating constraints list
/// and has_primary_key flag as needed. `bare_table_name` is the table's
/// unqualified name, which unnamed constraints and their backing indexes
/// are named after.
fn apply_table_constraint(
    table: &mut TableState,
    bare_table_name: &str,
    constraint: &TableConstraint,
    index_registry: &mut IndexRegistryUpdates,
) {
    match constraint {
        TableConstraint::PrimaryKey {
            name,
            columns,
            using_index,
            ..
        } => {
            table.has_primary_key = true;
            let name = index_backed_constraint_name(name, using_index, || {
                GeneratedName::primary_key(bare_table_name)
            });
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
            name,
            columns,
            ref_table,
            ref_columns,
            not_valid,
        } => {
            table.constraints.push(ConstraintState::ForeignKey {
                name: name.clone().unwrap_or_else(|| {
                    GeneratedName::foreign_key(bare_table_name, columns).unsuffixed()
                }),
                columns: columns.clone(),
                ref_table: ref_table.relation().clone(),
                ref_table_display: ref_table.display_name(),
                ref_columns: ref_columns.clone(),
                not_valid: *not_valid,
            });
        }
        TableConstraint::Unique {
            name,
            columns,
            include_columns,
            using_index,
        } => {
            let name = index_backed_constraint_name(name, using_index, || {
                GeneratedName::unique(bare_table_name, columns, include_columns)
            });
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
            name,
            expression,
            referenced_columns,
            not_valid,
        } => {
            table.constraints.push(ConstraintState::Check {
                name: name.clone().unwrap_or_else(|| {
                    GeneratedName::check(bare_table_name, referenced_columns).unsuffixed()
                }),
                expression: expression.clone(),
                not_valid: *not_valid,
            });
        }
        TableConstraint::Exclude {
            name,
            elements,
            include_columns,
        } => {
            table.constraints.push(ConstraintState::Exclude {
                name: name.clone().unwrap_or_else(|| {
                    GeneratedName::exclude(bare_table_name, elements, include_columns).unsuffixed()
                }),
                elements: elements.clone(),
            });
        }
    }
}
