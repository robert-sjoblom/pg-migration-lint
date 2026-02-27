//! Catalog replay engine — applies IR nodes to mutate the catalog.
//!
//! The replay engine processes migration units in order, applying each
//! IR statement to build up the table catalog. This is the core of the
//! single-pass replay strategy: the pipeline calls [`apply`] for each
//! migration unit, and the catalog accumulates state over time.

use crate::catalog::types::*;
use crate::input::MigrationUnit;
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
        IrNode::AlterIndexAttachPartition {
            parent_index_name, ..
        } => apply_alter_index_attach(catalog, parent_index_name),
        IrNode::DropSchema(ds) => apply_drop_schema(catalog, ds),
        IrNode::TruncateTable(_) | IrNode::Cluster(_) => { /* no schema state change */ }
        IrNode::InsertInto(_) | IrNode::UpdateTable(_) | IrNode::DeleteFrom(_) => {
            /* DML: no schema change */
        }
        IrNode::Unparseable { table_hint, .. } => apply_unparseable(catalog, table_hint),
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
    let table_key = ct.name.catalog_key().to_string();

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

    let parent_key = ct
        .partition_of
        .as_ref()
        .map(|p| p.catalog_key().to_string());

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

    // Convert columns (explicit columns on the child, or regular table columns)
    for col in &ct.columns {
        table.columns.push(column_def_to_state(col));

        // Handle inline PK on the column definition
        if col.is_inline_pk {
            apply_table_constraint(
                &mut table,
                &TableConstraint::PrimaryKey {
                    columns: vec![col.name.clone()],
                    using_index: None,
                },
            );
        }
    }

    // Convert table-level constraints
    for constraint in &ct.constraints {
        apply_table_constraint(&mut table, constraint);
    }

    catalog.insert_table(table);

    // Register as partition child only if parent exists in catalog.
    // If parent is not tracked, skip partition_children registration.
    if let Some(ref pk) = parent_key
        && catalog.has_table(pk)
    {
        catalog.attach_partition(pk, &table_key);
    }
}

/// Handle ALTER TABLE: apply each action to the existing table.
/// If the table does not exist in the catalog, silently skip.
fn apply_alter_table(catalog: &mut Catalog, at: &AlterTable) {
    let table_key = at.name.catalog_key().to_string();

    // If the table doesn't exist, silently skip. It may be in a different
    // schema or managed outside our tracked migrations.
    if !catalog.has_table(&table_key) {
        return;
    }

    // Collect indexes to register/unregister so we can update the reverse map
    // after releasing the mutable borrow on the table.
    let mut indexes_to_register: Vec<String> = Vec::new();
    let mut indexes_to_unregister: Vec<String> = Vec::new();

    {
        let Some(table) = catalog.get_table_mut(&table_key) else {
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
                            &TableConstraint::PrimaryKey {
                                columns: vec![col_def.name.clone()],
                                using_index: None,
                            },
                        );
                        indexes_to_register.push(format!("{}_pkey", table.name));
                    }
                }
                AlterTableAction::DropColumn { name } => {
                    // Collect index names that will be removed by the column drop.
                    // Uses references_column() to also detect expression indexes
                    // that reference the dropped column (e.g. `lower(email)`).
                    for idx in &table.indexes {
                        if idx.references_column(name) {
                            indexes_to_unregister.push(idx.name.clone());
                        }
                    }
                    table.remove_column(name);
                }
                AlterTableAction::AddConstraint(constraint) => {
                    // Track synthetic PK indexes created by apply_table_constraint.
                    // Only when there's no USING INDEX (with USING INDEX the index already exists).
                    if matches!(
                        constraint,
                        TableConstraint::PrimaryKey {
                            using_index: None,
                            ..
                        }
                    ) {
                        indexes_to_register.push(format!("{}_pkey", table.name));
                    }
                    apply_table_constraint(table, constraint);
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
                AlterTableAction::DropConstraint { constraint_name } => {
                    // Check if we're dropping a PK constraint.
                    // ConstraintState::PrimaryKey has no name field; PostgreSQL
                    // names it `{table}_pkey` by default.
                    let pkey_name = format!("{}_pkey", table.name);
                    let dropping_pk = table.has_primary_key && *constraint_name == pkey_name;

                    // Remove named constraints (FK, Unique, Check, Exclude).
                    table.constraints.retain(|c| match c {
                        ConstraintState::ForeignKey { name, .. }
                        | ConstraintState::Unique { name, .. }
                        | ConstraintState::Check { name, .. }
                        | ConstraintState::Exclude { name, .. } => {
                            name.as_deref() != Some(constraint_name)
                        }
                        ConstraintState::PrimaryKey { .. } => !dropping_pk,
                    });

                    if dropping_pk {
                        table.has_primary_key = false;
                        indexes_to_unregister.push(pkey_name.clone());
                        table.indexes.retain(|idx| idx.name != pkey_name);
                    }

                    // PostgreSQL drops the backing index when a constraint is
                    // dropped (UNIQUE, EXCLUDE, and PK all have backing indexes).
                    // Remove any index whose name matches the constraint name.
                    if table.indexes.iter().any(|idx| idx.name == *constraint_name) {
                        table.indexes.retain(|idx| idx.name != *constraint_name);
                        indexes_to_unregister.push(constraint_name.clone());
                    }
                }
                AlterTableAction::ValidateConstraint { constraint_name } => {
                    for c in &mut table.constraints {
                        match c {
                            ConstraintState::ForeignKey {
                                name, not_valid, ..
                            } if name.as_deref() == Some(constraint_name) => {
                                *not_valid = false;
                            }
                            ConstraintState::Check {
                                name, not_valid, ..
                            } if name.as_deref() == Some(constraint_name) => {
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
    for name in indexes_to_unregister {
        catalog.unregister_index(&name);
    }
    for name in indexes_to_register {
        catalog.register_index(&name, &table_key);
    }

    // Handle partition attach/detach outside the table borrow scope.
    for action in &at.actions {
        match action {
            AlterTableAction::AttachPartition { child } => {
                let child_key = child.catalog_key().to_string();
                if catalog.has_table(&child_key) {
                    if let Some(child_table) = catalog.get_table_mut(&child_key) {
                        child_table.parent_table = Some(table_key.clone());
                    }
                    catalog.attach_partition(&table_key, &child_key);
                }
                // If child not in catalog: skip silently (no phantom entry).
            }
            AlterTableAction::DetachPartition { child, .. } => {
                let child_key = child.catalog_key().to_string();
                if let Some(child_table) = catalog.get_table_mut(&child_key) {
                    child_table.parent_table = None;
                }
                catalog.detach_partition(&table_key, &child_key);
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
    let table_key = ci.table_name.catalog_key().to_string();

    let index_name = ci.index_name.clone().unwrap_or_default();

    if !index_name.is_empty() && catalog.get_index(&index_name).is_some() {
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

    let Some(table) = catalog.get_table_mut(&table_key) else {
        return;
    };

    let entries: Vec<IndexColumn> = ci.columns.to_vec();

    table.indexes.push(IndexState {
        name: index_name.clone(),
        entries,
        unique: ci.unique,
        where_clause: ci.where_clause.clone(),
        only: ci.only,
    });

    // Register after confirming the table exists, to avoid ghost entries.
    catalog.register_index(&index_name, &table_key);
}

/// Handle DROP INDEX: find and remove the named index from whichever table has it.
/// If no table has the index, silently skip.
fn apply_drop_index(catalog: &mut Catalog, di: &DropIndex) {
    let Some(table_name) = catalog.table_for_index(&di.index_name).map(str::to_string) else {
        return;
    };

    catalog.unregister_index(&di.index_name);

    if let Some(table) = catalog.get_table_mut(&table_name) {
        table.indexes.retain(|idx| idx.name != di.index_name);
    }
}

/// Handle ALTER INDEX ... ATTACH PARTITION: flip `only` to `false` on the parent index.
///
/// When all child partitions have their own indexes attached, the parent
/// ON ONLY index becomes a valid recursive index covering all partitions.
fn apply_alter_index_attach(catalog: &mut Catalog, parent_index_name: &str) {
    let Some(table_key) = catalog.table_for_index(parent_index_name).map(String::from) else {
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
/// For partitioned tables with CASCADE, recursively removes all partition
/// children (depth-first). Without CASCADE, children keep stale `parent_table`.
fn apply_drop_table(catalog: &mut Catalog, dt: &DropTable) {
    let table_key = dt.name.catalog_key().to_string();

    // If table is partitioned and CASCADE, recursively remove the partition subtree.
    if dt.cascade
        && let Some(table) = catalog.get_table(&table_key)
        && table.is_partitioned
    {
        let children_to_remove = collect_partition_subtree(catalog, &table_key);
        for child_key in children_to_remove {
            catalog.remove_table(&child_key);
        }
    }

    catalog.remove_table(&table_key);
}

/// Handle DROP SCHEMA: remove all tables in the schema from the catalog.
///
/// With CASCADE, all tables whose catalog key starts with `"{schema_name}."`
/// are removed. Without CASCADE, PostgreSQL would error at runtime if the
/// schema is non-empty, so we treat it as a no-op.
fn apply_drop_schema(catalog: &mut Catalog, ds: &DropSchema) {
    if !ds.cascade {
        return;
    }

    let prefix = format!("{}.", ds.schema_name);
    let keys_to_remove: Vec<String> = catalog
        .tables()
        .filter(|t| t.name.starts_with(&prefix))
        .map(|t| t.name.clone())
        .collect();

    for key in keys_to_remove {
        catalog.remove_table(&key);
    }
}

/// Collect all partition children recursively (depth-first) for cascade removal.
/// Uses a visited set to prevent cycles.
fn collect_partition_subtree(catalog: &Catalog, root_key: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut stack = vec![root_key.to_string()];
    let mut visited = std::collections::HashSet::new();

    while let Some(key) = stack.pop() {
        if !visited.insert(key.clone()) {
            continue;
        }
        let children = catalog.get_partition_children(&key);
        for child in children {
            result.push(child.clone());
            stack.push(child.clone());
        }
    }

    result
}

/// Handle RENAME TABLE: move the table state to a new key.
///
/// Also updates partition tracking:
/// - If the renamed table is a partitioned parent: migrates `partition_children`
///   to the new key and updates each child's `parent_table`.
/// - If the renamed table is a partition child: updates the parent's
///   `partition_children` list (old key was already removed by `remove_table`;
///   new key is re-added).
fn apply_rename_table(catalog: &mut Catalog, name: &QualifiedName, new_name: &str) {
    let old_key = name.catalog_key().to_string();

    // Grab partition children BEFORE remove_table, which cleans up partition_children.
    let partition_children = catalog.remove_partition_children(&old_key);

    if let Some(mut table) = catalog.remove_table(&old_key) {
        // Build the new key using the same schema as the old name.
        let new_key = match &name.schema {
            Some(schema) => format!("{}.{}", schema, new_name),
            None => {
                // After normalization, schema should always be set. Fall back to
                // extracting it from the old catalog key ("schema.name" format).
                if let Some(dot) = old_key.find('.') {
                    format!("{}.{}", &old_key[..dot], new_name)
                } else {
                    new_name.to_string()
                }
            }
        };

        // Migrate partition_children from old key to new key, and update
        // each child's parent_table reference.
        if table.is_partitioned
            && let Some(children) = partition_children
        {
            for child_key in &children {
                if let Some(child) = catalog.get_table_mut(child_key) {
                    child.parent_table = Some(new_key.clone());
                }
            }
            catalog.set_partition_children(&new_key, children);
        }

        // If the renamed table is a child, re-register under the new key
        // in the parent's partition_children list. (remove_table already
        // removed the old key from the parent's list.)
        if let Some(ref parent_key) = table.parent_table {
            catalog.attach_partition(parent_key, &new_key);
        }

        table.name = new_key.clone();
        table.display_name = new_name.to_string();
        catalog.insert_table(table);
    }
}

/// Handle RENAME COLUMN: rename a column in a table, updating indexes and constraints.
fn apply_rename_column(
    catalog: &mut Catalog,
    table_name: &QualifiedName,
    old_name: &str,
    new_name: &str,
) {
    let table_key = table_name.catalog_key().to_string();
    let Some(table) = catalog.get_table_mut(&table_key) else {
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
        for entry in &mut idx.entries {
            match entry {
                IndexColumn::Column(col) => {
                    if *col == old_name {
                        *col = new_name.to_string();
                    }
                }
                IndexColumn::Expression {
                    referenced_columns, ..
                } => {
                    for col in referenced_columns {
                        if *col == old_name {
                            *col = new_name.to_string();
                        }
                    }
                }
            }
        }
    }

    // Update constraints that reference the old column name.
    let table_name_key = table.name.clone();
    for constraint in &mut table.constraints {
        match constraint {
            ConstraintState::PrimaryKey { columns } | ConstraintState::Unique { columns, .. } => {
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
            // TODO: EXCLUDE constraints reference columns (e.g. `room WITH =`),
            // but the IR does not capture them — no column list to rename.
            ConstraintState::Exclude { .. } => {}
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
fn apply_unparseable(catalog: &mut Catalog, table_hint: &Option<String>) {
    if let Some(hint) = table_hint
        && let Some(table) = catalog.get_table_mut(hint)
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

/// Apply a table constraint to the table state, updating constraints list
/// and has_primary_key flag as needed.
fn apply_table_constraint(table: &mut TableState, constraint: &TableConstraint) {
    match constraint {
        TableConstraint::PrimaryKey {
            columns,
            using_index,
        } => {
            table.has_primary_key = true;
            // When USING INDEX is set, IR columns are empty — resolve from the
            // referenced index so downstream rules (e.g. PGM014) see the real columns.
            // Expression entries are intentionally excluded (column_names() skips them)
            // since PK constraints reference plain columns; in practice PostgreSQL
            // rejects ADD PRIMARY KEY USING INDEX on expression indexes.
            let resolved_columns = if columns.is_empty() {
                using_index
                    .as_ref()
                    .and_then(|idx_name| {
                        table
                            .indexes
                            .iter()
                            .find(|idx| idx.name == *idx_name)
                            .map(|idx| idx.column_names().map(str::to_string).collect())
                    })
                    .unwrap_or_default()
            } else {
                columns.clone()
            };
            table.constraints.push(ConstraintState::PrimaryKey {
                columns: resolved_columns,
            });
            // Only create a synthetic PK index when there's no USING INDEX.
            // With USING INDEX, the referenced index already exists in the catalog.
            if using_index.is_none() {
                table.indexes.push(IndexState {
                    name: format!("{}_pkey", table.name),
                    entries: columns
                        .iter()
                        .map(|c| IndexColumn::Column(c.clone()))
                        .collect(),
                    unique: true,
                    where_clause: None,
                    only: false,
                });
            }
        }
        TableConstraint::ForeignKey {
            name,
            columns,
            ref_table,
            ref_columns,
            not_valid,
        } => {
            table.constraints.push(ConstraintState::ForeignKey {
                name: name.clone(),
                columns: columns.clone(),
                ref_table: ref_table.catalog_key().to_string(),
                ref_table_display: ref_table.display_name(),
                ref_columns: ref_columns.clone(),
                not_valid: *not_valid,
            });
        }
        TableConstraint::Unique {
            name,
            columns,
            using_index,
        } => {
            // When USING INDEX is set, IR columns are empty — resolve from the
            // referenced index so downstream constraint checks see the real columns.
            let resolved_columns = if columns.is_empty() {
                using_index
                    .as_ref()
                    .and_then(|idx_name| {
                        table
                            .indexes
                            .iter()
                            .find(|idx| idx.name == *idx_name)
                            .map(|idx| idx.column_names().map(str::to_string).collect())
                    })
                    .unwrap_or_default()
            } else {
                columns.clone()
            };
            table.constraints.push(ConstraintState::Unique {
                name: name.clone(),
                columns: resolved_columns,
            });
        }
        TableConstraint::Check {
            name,
            expression,
            not_valid,
        } => {
            table.constraints.push(ConstraintState::Check {
                name: name.clone(),
                expression: expression.clone(),
                not_valid: *not_valid,
            });
        }
        TableConstraint::Exclude { name } => {
            table
                .constraints
                .push(ConstraintState::Exclude { name: name.clone() });
        }
    }
}
