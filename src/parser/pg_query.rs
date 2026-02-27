//! pg_query AST to IR conversion
//!
//! This module converts the pg_query crate's PostgreSQL AST into the simplified
//! IR layer used by the rule engine. It handles type canonicalization, constraint
//! normalization, and source location tracking.

use crate::parser::ir::{
    AlterTable, AlterTableAction, Cluster, ColumnDef, CreateIndex, CreateTable, DefaultExpr,
    DeleteFrom, DropIndex, DropSchema, DropTable, IndexColumn, InsertInto, IrNode, Located,
    PartitionBy, PartitionStrategy, QualifiedName, SourceSpan, TableConstraint, TablePersistence,
    TriggerDisableScope, TruncateTable, TypeName, UpdateTable,
};
use pg_query::NodeEnum;

/// Sentinel type name used when the actual type cannot be determined.
const UNKNOWN_TYPE: &str = "unknown";

#[cfg(test)]
mod tests;

/// Parse a SQL source string into a list of located IR nodes.
///
/// Each SQL statement in the source is converted to the most specific IR node
/// possible. Statements that fail to parse entirely are returned as a single
/// `Unparseable` node. Statements that parse but have no IR mapping (e.g.,
/// GRANT, COMMENT ON) are returned as `Ignored`.
///
/// Line numbers in the returned `SourceSpan`s are 1-based.
pub fn parse_sql(source: &str) -> Vec<Located<IrNode>> {
    let result = match pg_query::parse(source) {
        Ok(r) => r,
        Err(_) => {
            // Entire source failed to parse; return a single Unparseable node
            let end_line = source.lines().count().max(1);
            return vec![Located {
                node: IrNode::Unparseable {
                    raw_sql: source.to_string(),
                    table_hint: extract_table_hint_from_raw(source),
                },
                span: SourceSpan {
                    start_line: 1,
                    end_line,
                    start_offset: 0,
                    end_offset: source.len(),
                },
            }];
        }
    };

    let mut nodes = Vec::new();

    for raw_stmt in &result.protobuf.stmts {
        let start_offset = raw_stmt.stmt_location as usize;
        let end_offset = if raw_stmt.stmt_len > 0 {
            start_offset + raw_stmt.stmt_len as usize
        } else {
            source.len()
        };
        // pg_query may include leading whitespace (including newlines) in
        // stmt_location. Skip it to find the actual first token for accurate
        // line number reporting.
        let token_start = source[start_offset..]
            .find(|c: char| !c.is_whitespace())
            .map(|i| start_offset + i)
            .unwrap_or(start_offset);
        let start_line = byte_offset_to_line(source, token_start);
        let end_line = byte_offset_to_line(source, end_offset.saturating_sub(1).max(start_offset));

        let raw_sql = source
            .get(start_offset..end_offset)
            .unwrap_or("")
            .to_string();

        let stmt_node = raw_stmt.stmt.as_ref().and_then(|s| s.node.as_ref());

        let ir_nodes = match stmt_node {
            Some(node_enum) => convert_node(node_enum, &raw_sql),
            None => vec![IrNode::Ignored {
                raw_sql: raw_sql.clone(),
            }],
        };

        let span = SourceSpan {
            start_line,
            end_line,
            start_offset,
            end_offset,
        };

        for ir_node in ir_nodes {
            nodes.push(Located {
                node: ir_node,
                span: span.clone(),
            });
        }
    }

    nodes
}

/// Convert a byte offset into a 1-based line number.
///
/// Counts the number of newline characters in `source[..offset]` and adds 1.
fn byte_offset_to_line(source: &str, offset: usize) -> usize {
    let clamped = offset.min(source.len());
    source[..clamped].matches('\n').count() + 1
}

/// Convert a pg_query `NodeEnum` into one or more IR nodes.
///
/// Most statements produce a single IR node. Multi-target statements like
/// `DROP TABLE t1, t2` or `TRUNCATE t1, t2, t3 CASCADE` produce one node
/// per target table.
fn convert_node(node: &NodeEnum, raw_sql: &str) -> Vec<IrNode> {
    match node {
        NodeEnum::CreateStmt(create) => vec![convert_create_table(create, raw_sql)],
        NodeEnum::AlterTableStmt(alter) => {
            if alter.objtype() == pg_query::protobuf::ObjectType::ObjectIndex {
                convert_alter_index(alter, raw_sql)
            } else {
                vec![convert_alter_table(alter, raw_sql)]
            }
        }
        NodeEnum::IndexStmt(idx) => vec![convert_create_index(idx)],
        NodeEnum::DropStmt(drop) => convert_drop_stmt(drop, raw_sql),
        NodeEnum::RenameStmt(rename) => vec![convert_rename_stmt(rename, raw_sql)],
        NodeEnum::TruncateStmt(trunc) => convert_truncate_stmt(trunc),
        NodeEnum::InsertStmt(insert) => vec![convert_insert_stmt(insert)],
        NodeEnum::UpdateStmt(update) => vec![convert_update_stmt(update)],
        NodeEnum::DeleteStmt(delete) => vec![convert_delete_stmt(delete)],
        NodeEnum::ClusterStmt(cluster) => vec![convert_cluster_stmt(cluster)],
        NodeEnum::DoStmt(_) => vec![IrNode::Unparseable {
            raw_sql: raw_sql.to_string(),
            table_hint: None,
        }],
        _ => vec![IrNode::Ignored {
            raw_sql: raw_sql.to_string(),
        }],
    }
}

// ---------------------------------------------------------------------------
// CREATE TABLE
// ---------------------------------------------------------------------------

/// Convert a pg_query `CreateStmt` to `IrNode::CreateTable`.
fn convert_create_table(create: &pg_query::protobuf::CreateStmt, _raw_sql: &str) -> IrNode {
    let name = relation_to_qualified_name(create.relation.as_ref());

    let persistence = if matches!(
        create.oncommit(),
        pg_query::protobuf::OnCommitAction::OncommitDrop
            | pg_query::protobuf::OnCommitAction::OncommitDeleteRows
    ) {
        TablePersistence::Temporary
    } else {
        relation_persistence(create.relation.as_ref())
    };

    let mut columns = Vec::new();
    let mut constraints = Vec::new();

    for elt in &create.table_elts {
        let node = match elt.node.as_ref() {
            Some(n) => n,
            None => continue,
        };

        match node {
            NodeEnum::ColumnDef(col) => {
                let (col_def, inline_constraints) = convert_column_def(col);
                columns.push(col_def);
                constraints.extend(inline_constraints);
            }
            NodeEnum::Constraint(con) => {
                if let Some(tc) = convert_table_constraint(con, None) {
                    constraints.push(tc);
                }
            }
            _ => {}
        }
    }

    // Extract PARTITION BY clause
    let partition_by = create.partspec.as_ref().and_then(|spec| {
        let strategy = match pg_query::protobuf::PartitionStrategy::try_from(spec.strategy) {
            Ok(pg_query::protobuf::PartitionStrategy::Range) => Some(PartitionStrategy::Range),
            Ok(pg_query::protobuf::PartitionStrategy::List) => Some(PartitionStrategy::List),
            Ok(pg_query::protobuf::PartitionStrategy::Hash) => Some(PartitionStrategy::Hash),
            _ => None,
        }?;
        let part_columns: Vec<String> = spec
            .part_params
            .iter()
            .filter_map(|p| match p.node.as_ref() {
                Some(NodeEnum::PartitionElem(elem)) => {
                    if !elem.name.is_empty() {
                        Some(elem.name.clone())
                    } else {
                        // Expression partition key — deparse the expression text.
                        elem.expr.as_ref().map(|expr_node| deparse_node(expr_node))
                    }
                }
                _ => None,
            })
            .collect();
        Some(PartitionBy {
            strategy,
            columns: part_columns,
        })
    });

    // Extract PARTITION OF parent (when partbound is present, inh_relations[0] is the parent)
    let partition_of = if create.partbound.is_some() {
        create
            .inh_relations
            .first()
            .and_then(|node| match node.node.as_ref() {
                Some(NodeEnum::RangeVar(rv)) => Some(relation_to_qualified_name(Some(rv))),
                _ => None,
            })
    } else {
        None
    };

    IrNode::CreateTable(CreateTable {
        name,
        columns,
        constraints,
        persistence,
        if_not_exists: create.if_not_exists,
        partition_by,
        partition_of,
    })
}

/// Convert a constraint name to `Option<String>`, treating empty strings as `None`.
fn optional_name(name: &str) -> Option<String> {
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Convert a pg_query `ColumnDef` into an IR `ColumnDef` plus any inline
/// constraints that should be promoted to table-level constraints.
///
/// Returns `(ColumnDef, Vec<TableConstraint>)` where the vector contains
/// inline PRIMARY KEY, FOREIGN KEY, UNIQUE, and CHECK constraints.
fn convert_column_def(col: &pg_query::protobuf::ColumnDef) -> (ColumnDef, Vec<TableConstraint>) {
    let col_name = col.colname.clone();

    // Extract type name
    let (type_name, is_serial) = extract_type_name(col.type_name.as_ref());

    let mut nullable = true;
    let mut default_expr = None;
    let mut is_inline_pk = false;
    let mut constraints = Vec::new();

    // serial/bigserial implies a nextval() default
    if is_serial {
        default_expr = Some(DefaultExpr::FunctionCall {
            name: "nextval".to_string(),
            args: vec![],
        });
    }

    // Walk column constraints
    for con_node in &col.constraints {
        let con = match con_node.node.as_ref() {
            Some(NodeEnum::Constraint(c)) => c,
            _ => continue,
        };

        match con.contype() {
            pg_query::protobuf::ConstrType::ConstrNotnull => {
                nullable = false;
            }
            pg_query::protobuf::ConstrType::ConstrDefault => {
                if let Some(ref expr) = con.raw_expr {
                    default_expr = Some(convert_default_expr(expr));
                }
            }
            pg_query::protobuf::ConstrType::ConstrPrimary => {
                is_inline_pk = true;
                nullable = false;
                constraints.push(TableConstraint::PrimaryKey {
                    columns: vec![col_name.clone()],
                    using_index: None,
                });
            }
            pg_query::protobuf::ConstrType::ConstrForeign => {
                let ref_table = relation_to_qualified_name(con.pktable.as_ref());
                let ref_columns = extract_string_list(&con.pk_attrs);
                constraints.push(TableConstraint::ForeignKey {
                    name: optional_name(&con.conname),
                    columns: vec![col_name.clone()],
                    ref_table,
                    ref_columns,
                    not_valid: con.skip_validation,
                });
            }
            pg_query::protobuf::ConstrType::ConstrUnique => {
                constraints.push(TableConstraint::Unique {
                    name: optional_name(&con.conname),
                    columns: vec![col_name.clone()],
                    using_index: None,
                });
            }
            pg_query::protobuf::ConstrType::ConstrCheck => {
                let expression = con
                    .raw_expr
                    .as_ref()
                    .map(|e| deparse_node(e))
                    .unwrap_or_default();
                constraints.push(TableConstraint::Check {
                    name: optional_name(&con.conname),
                    expression,
                    not_valid: con.skip_validation,
                });
            }
            _ => {}
        }
    }

    let col_def = ColumnDef {
        name: col_name,
        type_name,
        nullable,
        default_expr,
        is_inline_pk,
        is_serial,
    };

    (col_def, constraints)
}

// ---------------------------------------------------------------------------
// Type name extraction
// ---------------------------------------------------------------------------

/// Extract a canonical `TypeName` from a pg_query `TypeName` node.
///
/// Returns `(TypeName, is_serial)` where `is_serial` is true if the original
/// type was `smallserial`, `serial`, or `bigserial` (which pg_query does NOT expand).
///
/// Canonical name extraction: use the LAST element of `TypeName.names[]`.
/// This normalizes all PostgreSQL type aliases automatically.
fn extract_type_name(tn: Option<&pg_query::protobuf::TypeName>) -> (TypeName, bool) {
    let tn = match tn {
        Some(t) => t,
        None => return (TypeName::simple(UNKNOWN_TYPE), false),
    };

    // Extract the last string from names[]
    let canonical = tn
        .names
        .iter()
        .rev()
        .find_map(|n| match n.node.as_ref() {
            Some(NodeEnum::String(s)) => Some(s.sval.clone()),
            _ => None,
        })
        .unwrap_or_else(|| UNKNOWN_TYPE.to_string())
        .to_lowercase();

    // Check for serial types (NOT expanded by pg_query)
    let is_serial = matches!(canonical.as_str(), "smallserial" | "serial" | "bigserial");
    let mapped_name = match canonical.as_str() {
        "smallserial" => "int2".to_string(),
        "serial" => "int4".to_string(),
        "bigserial" => "int8".to_string(),
        other => other.to_string(),
    };

    // Extract type modifiers from typmods[]
    let modifiers = extract_type_modifiers(&tn.typmods);

    let type_name = if modifiers.is_empty() {
        TypeName::simple(mapped_name)
    } else {
        TypeName::with_modifiers(mapped_name, modifiers)
    };

    (type_name, is_serial)
}

/// Extract integer modifiers from `TypeName.typmods[]`.
///
/// Type modifiers appear as `AConst(Integer)` nodes. For example:
/// - `varchar(100)` has modifiers `[100]`
/// - `numeric(10,2)` has modifiers `[10, 2]`
fn extract_type_modifiers(typmods: &[pg_query::protobuf::Node]) -> Vec<i64> {
    let mut mods = Vec::new();
    for node in typmods {
        if let Some(ref inner) = node.node {
            match inner {
                NodeEnum::Integer(i) => {
                    mods.push(i.ival as i64);
                }
                NodeEnum::AConst(ac) => {
                    if let Some(pg_query::protobuf::a_const::Val::Ival(i)) = &ac.val {
                        mods.push(i.ival as i64);
                    }
                }
                _ => {}
            }
        }
    }
    mods
}

// ---------------------------------------------------------------------------
// Default expressions
// ---------------------------------------------------------------------------

/// Convert a pg_query expression node into an IR `DefaultExpr`.
///
/// Mapping:
/// - `AConst` (Integer, String, Boolean) -> `DefaultExpr::Literal`
/// - `FuncCall` -> `DefaultExpr::FunctionCall`
/// - Everything else -> `DefaultExpr::Other`
fn convert_default_expr(node: &pg_query::protobuf::Node) -> DefaultExpr {
    match node.node.as_ref() {
        Some(NodeEnum::AConst(ac)) => {
            let literal_str = match ac.val.as_ref() {
                Some(pg_query::protobuf::a_const::Val::Ival(i)) => i.ival.to_string(),
                Some(pg_query::protobuf::a_const::Val::Sval(s)) => s.sval.clone(),
                Some(pg_query::protobuf::a_const::Val::Boolval(b)) => {
                    if b.boolval { "true" } else { "false" }.to_string()
                }
                Some(pg_query::protobuf::a_const::Val::Fval(f)) => f.fval.clone(),
                Some(pg_query::protobuf::a_const::Val::Bsval(s)) => s.bsval.clone(),
                None => "NULL".to_string(),
            };
            DefaultExpr::Literal(literal_str)
        }
        Some(NodeEnum::FuncCall(fc)) => {
            let name = fc
                .funcname
                .iter()
                .rev()
                .find_map(|n| match n.node.as_ref() {
                    Some(NodeEnum::String(s)) => Some(s.sval.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| "unknown".to_string());

            let args: Vec<String> = fc.args.iter().map(deparse_node).collect();

            DefaultExpr::FunctionCall { name, args }
        }
        Some(NodeEnum::TypeCast(_)) => DefaultExpr::Other(deparse_node(node)),
        Some(_) => DefaultExpr::Other(deparse_node(node)),
        None => DefaultExpr::Other("NULL".to_string()),
    }
}

// ---------------------------------------------------------------------------
// ALTER TABLE
// ---------------------------------------------------------------------------

/// Convert a pg_query `AlterTableStmt` to `IrNode::AlterTable`.
fn convert_alter_table(alter: &pg_query::protobuf::AlterTableStmt, raw_sql: &str) -> IrNode {
    let name = relation_to_qualified_name(alter.relation.as_ref());

    let mut actions = Vec::new();

    for cmd_node in &alter.cmds {
        let cmd = match cmd_node.node.as_ref() {
            Some(NodeEnum::AlterTableCmd(c)) => c,
            _ => continue,
        };

        let new_actions = convert_alter_table_cmd(cmd);
        actions.extend(new_actions);
    }

    if actions.is_empty() {
        return IrNode::Ignored {
            raw_sql: raw_sql.to_string(),
        };
    }

    IrNode::AlterTable(AlterTable { name, actions })
}

/// Convert a single `AlterTableCmd` into one or more `AlterTableAction`s.
///
/// Returns a `Vec` because `ADD COLUMN` with inline constraints (e.g. FK,
/// UNIQUE, CHECK) produces the column action *plus* constraint actions.
fn convert_alter_table_cmd(cmd: &pg_query::protobuf::AlterTableCmd) -> Vec<AlterTableAction> {
    match cmd.subtype() {
        pg_query::protobuf::AlterTableType::AtAddColumn => {
            match cmd.def.as_ref().and_then(|d| d.node.as_ref()) {
                Some(NodeEnum::ColumnDef(col)) => {
                    let (col_def, inline_constraints) = convert_column_def(col);
                    let mut result = vec![AlterTableAction::AddColumn(col_def)];
                    result.extend(
                        inline_constraints
                            .into_iter()
                            .map(AlterTableAction::AddConstraint),
                    );
                    result
                }
                _ => vec![AlterTableAction::Other {
                    description: "ADD COLUMN (unparseable definition)".to_string(),
                }],
            }
        }
        pg_query::protobuf::AlterTableType::AtDropColumn => vec![AlterTableAction::DropColumn {
            name: cmd.name.clone(),
        }],
        pg_query::protobuf::AlterTableType::AtAddConstraint => {
            match cmd.def.as_ref().and_then(|d| d.node.as_ref()) {
                Some(NodeEnum::Constraint(con)) => match convert_table_constraint(con, None) {
                    Some(tc) => vec![AlterTableAction::AddConstraint(tc)],
                    None => vec![AlterTableAction::Other {
                        description: "ADD CONSTRAINT (unknown type)".to_string(),
                    }],
                },
                _ => vec![AlterTableAction::Other {
                    description: "ADD CONSTRAINT (unparseable)".to_string(),
                }],
            }
        }
        pg_query::protobuf::AlterTableType::AtAlterColumnType => {
            // The new type is in cmd.def as a ColumnDef with the type_name
            let new_type = cmd
                .def
                .as_ref()
                .and_then(|d| d.node.as_ref())
                .and_then(|n| match n {
                    NodeEnum::ColumnDef(col) => Some(extract_type_name(col.type_name.as_ref()).0),
                    _ => None,
                })
                .unwrap_or_else(|| TypeName::simple(UNKNOWN_TYPE));

            vec![AlterTableAction::AlterColumnType {
                column_name: cmd.name.clone(),
                new_type,
                old_type: None, // Must be filled in from catalog during linting
            }]
        }
        pg_query::protobuf::AlterTableType::AtSetNotNull => {
            vec![AlterTableAction::SetNotNull {
                column_name: cmd.name.clone(),
            }]
        }
        pg_query::protobuf::AlterTableType::AtDropNotNull => vec![AlterTableAction::Other {
            description: format!("DROP NOT NULL on {}", cmd.name),
        }],
        pg_query::protobuf::AlterTableType::AtAttachPartition => {
            let child = cmd
                .def
                .as_ref()
                .and_then(|d| d.node.as_ref())
                .and_then(|n| match n {
                    NodeEnum::PartitionCmd(pc) => pc
                        .name
                        .as_ref()
                        .map(|rv| relation_to_qualified_name(Some(rv))),
                    _ => None,
                });
            match child {
                Some(child) => vec![AlterTableAction::AttachPartition { child }],
                None => vec![AlterTableAction::Other {
                    description: "ATTACH PARTITION (unparseable)".to_string(),
                }],
            }
        }
        pg_query::protobuf::AlterTableType::AtDetachPartition => {
            let result = cmd
                .def
                .as_ref()
                .and_then(|d| d.node.as_ref())
                .and_then(|n| match n {
                    NodeEnum::PartitionCmd(pc) => pc
                        .name
                        .as_ref()
                        .map(|rv| (relation_to_qualified_name(Some(rv)), pc.concurrent)),
                    _ => None,
                });
            match result {
                Some((child, concurrent)) => {
                    vec![AlterTableAction::DetachPartition { child, concurrent }]
                }
                None => vec![AlterTableAction::Other {
                    description: "DETACH PARTITION (unparseable)".to_string(),
                }],
            }
        }
        pg_query::protobuf::AlterTableType::AtDisableTrig => {
            let scope = if cmd.name.is_empty() {
                TriggerDisableScope::All
            } else {
                TriggerDisableScope::Named(cmd.name.clone())
            };
            vec![AlterTableAction::DisableTrigger { scope }]
        }
        pg_query::protobuf::AlterTableType::AtDisableTrigAll => {
            vec![AlterTableAction::DisableTrigger {
                scope: TriggerDisableScope::All,
            }]
        }
        pg_query::protobuf::AlterTableType::AtDisableTrigUser => {
            vec![AlterTableAction::DisableTrigger {
                scope: TriggerDisableScope::User,
            }]
        }
        // ENABLE TRIGGER variants — not flagged, no schema state change.
        pg_query::protobuf::AlterTableType::AtEnableTrig
        | pg_query::protobuf::AlterTableType::AtEnableTrigAll
        | pg_query::protobuf::AlterTableType::AtEnableTrigUser
        | pg_query::protobuf::AlterTableType::AtEnableAlwaysTrig
        | pg_query::protobuf::AlterTableType::AtEnableReplicaTrig => {
            vec![AlterTableAction::Other {
                description: format!("{:?}", cmd.subtype()),
            }]
        }
        other => vec![AlterTableAction::Other {
            description: format!("{:?}", other),
        }],
    }
}

// ---------------------------------------------------------------------------
// ALTER INDEX
// ---------------------------------------------------------------------------

/// Convert a pg_query `AlterTableStmt` with `objtype = ObjectIndex` to IR nodes.
///
/// pg_query represents `ALTER INDEX` as `AlterTableStmt` with `objtype = ObjectIndex`.
/// We only model `ATTACH PARTITION`; all other ALTER INDEX subtypes are ignored.
fn convert_alter_index(alter: &pg_query::protobuf::AlterTableStmt, raw_sql: &str) -> Vec<IrNode> {
    let parent_name = match alter.relation.as_ref() {
        Some(r) => r.relname.clone(),
        None => {
            return vec![IrNode::Ignored {
                raw_sql: raw_sql.to_string(),
            }];
        }
    };

    for cmd_node in &alter.cmds {
        let cmd = match cmd_node.node.as_ref() {
            Some(NodeEnum::AlterTableCmd(c)) => c,
            _ => continue,
        };

        if cmd.subtype() == pg_query::protobuf::AlterTableType::AtAttachPartition {
            let child = cmd
                .def
                .as_ref()
                .and_then(|d| d.node.as_ref())
                .and_then(|n| match n {
                    NodeEnum::PartitionCmd(pc) => pc
                        .name
                        .as_ref()
                        .map(|rv| relation_to_qualified_name(Some(rv))),
                    _ => None,
                });
            if let Some(child_index_name) = child {
                return vec![IrNode::AlterIndexAttachPartition {
                    parent_index_name: parent_name,
                    child_index_name,
                }];
            }
        }
    }

    // All other ALTER INDEX subtypes (SET, RESET, SET TABLESPACE, etc.)
    vec![IrNode::Ignored {
        raw_sql: raw_sql.to_string(),
    }]
}

// ---------------------------------------------------------------------------
// RENAME TABLE / RENAME COLUMN
// ---------------------------------------------------------------------------

/// Convert a pg_query `RenameStmt` to the appropriate IR node.
///
/// - `ObjectType::ObjectTable` with no `subname` → `IrNode::RenameTable`
/// - `ObjectType::ObjectColumn` → `IrNode::RenameColumn`
/// - Everything else → `IrNode::Ignored`
fn convert_rename_stmt(rename: &pg_query::protobuf::RenameStmt, raw_sql: &str) -> IrNode {
    match rename.rename_type() {
        pg_query::protobuf::ObjectType::ObjectTable => {
            let name = relation_to_qualified_name(rename.relation.as_ref());
            IrNode::RenameTable {
                name,
                new_name: rename.newname.clone(),
            }
        }
        pg_query::protobuf::ObjectType::ObjectColumn => {
            let table = relation_to_qualified_name(rename.relation.as_ref());
            IrNode::RenameColumn {
                table,
                old_name: rename.subname.clone(),
                new_name: rename.newname.clone(),
            }
        }
        _ => IrNode::Ignored {
            raw_sql: raw_sql.to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Constraints
// ---------------------------------------------------------------------------

/// Convert a pg_query `Constraint` node into an IR `TableConstraint`.
///
/// `context_column` is the column name when converting an inline constraint
/// on a ColumnDef (used to set the FK's referencing column). For table-level
/// constraints this is `None`.
fn convert_table_constraint(
    con: &pg_query::protobuf::Constraint,
    context_column: Option<&str>,
) -> Option<TableConstraint> {
    let name = optional_name(&con.conname);

    match con.contype() {
        pg_query::protobuf::ConstrType::ConstrPrimary => {
            let mut columns = extract_string_list(&con.keys);
            if columns.is_empty()
                && let Some(col) = context_column
            {
                columns.push(col.to_string());
            }
            Some(TableConstraint::PrimaryKey {
                columns,
                using_index: optional_name(&con.indexname),
            })
        }
        pg_query::protobuf::ConstrType::ConstrForeign => {
            let ref_table = relation_to_qualified_name(con.pktable.as_ref());
            let ref_columns = extract_string_list(&con.pk_attrs);
            let mut columns = extract_string_list(&con.fk_attrs);
            if columns.is_empty()
                && let Some(col) = context_column
            {
                columns.push(col.to_string());
            }
            Some(TableConstraint::ForeignKey {
                name,
                columns,
                ref_table,
                ref_columns,
                not_valid: con.skip_validation,
            })
        }
        pg_query::protobuf::ConstrType::ConstrUnique => {
            let mut columns = extract_string_list(&con.keys);
            if columns.is_empty()
                && let Some(col) = context_column
            {
                columns.push(col.to_string());
            }
            Some(TableConstraint::Unique {
                name,
                columns,
                using_index: optional_name(&con.indexname),
            })
        }
        pg_query::protobuf::ConstrType::ConstrCheck => {
            let expression = con
                .raw_expr
                .as_ref()
                .map(|e| deparse_node(e))
                .unwrap_or_default();
            Some(TableConstraint::Check {
                name,
                expression,
                not_valid: con.skip_validation,
            })
        }
        pg_query::protobuf::ConstrType::ConstrExclusion => Some(TableConstraint::Exclude { name }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// CREATE INDEX
// ---------------------------------------------------------------------------

/// Convert a pg_query `IndexStmt` to `IrNode::CreateIndex`.
fn convert_create_index(idx: &pg_query::protobuf::IndexStmt) -> IrNode {
    let table_name = relation_to_qualified_name(idx.relation.as_ref());

    let index_name = if idx.idxname.is_empty() {
        None
    } else {
        Some(idx.idxname.clone())
    };

    let columns: Vec<IndexColumn> = idx
        .index_params
        .iter()
        .filter_map(|p| match p.node.as_ref() {
            Some(NodeEnum::IndexElem(elem)) => {
                if !elem.name.is_empty() {
                    // Simple column reference.
                    Some(IndexColumn::Column(elem.name.clone()))
                } else {
                    // Expression index element — deparse the expression and extract
                    // column references for DROP/RENAME tracking.
                    elem.expr.as_ref().map(|expr_node| IndexColumn::Expression {
                        text: deparse_node(expr_node),
                        referenced_columns: extract_column_refs(expr_node),
                    })
                }
            }
            _ => None,
        })
        .collect();

    let where_clause = idx.where_clause.as_ref().map(|node| deparse_node(node));

    // `inh=true` means normal inheritance (indexes propagate to children),
    // `inh=false` means ONLY (index on parent only, not propagated).
    let only = idx.relation.as_ref().map(|r| !r.inh).unwrap_or(false);

    IrNode::CreateIndex(CreateIndex {
        index_name,
        table_name,
        columns,
        unique: idx.unique,
        concurrent: idx.concurrent,
        if_not_exists: idx.if_not_exists,
        where_clause,
        only,
    })
}

// ---------------------------------------------------------------------------
// DROP statements
// ---------------------------------------------------------------------------

/// Convert a pg_query `DropStmt` to the appropriate IR node(s).
///
/// Multi-target statements like `DROP TABLE t1, t2` produce one IR node per
/// target, fixing the previous behavior of only handling the first object.
///
/// - `ObjectType::ObjectIndex` -> `IrNode::DropIndex` (one per index)
/// - `ObjectType::ObjectTable` -> `IrNode::DropTable` (one per table)
/// - Everything else -> `IrNode::Ignored`
fn convert_drop_stmt(drop: &pg_query::protobuf::DropStmt, raw_sql: &str) -> Vec<IrNode> {
    match drop.remove_type() {
        pg_query::protobuf::ObjectType::ObjectIndex => {
            let names = extract_all_names_from_drop_objects(&drop.objects);
            if names.is_empty() {
                return vec![IrNode::Ignored {
                    raw_sql: raw_sql.to_string(),
                }];
            }
            names
                .into_iter()
                .map(|name| {
                    IrNode::DropIndex(DropIndex {
                        index_name: name,
                        concurrent: drop.concurrent,
                        if_exists: drop.missing_ok,
                    })
                })
                .collect()
        }
        pg_query::protobuf::ObjectType::ObjectTable => {
            let qualified_names = extract_all_qualified_names_from_drop_objects(&drop.objects);
            if qualified_names.is_empty() {
                return vec![IrNode::Ignored {
                    raw_sql: raw_sql.to_string(),
                }];
            }
            qualified_names
                .into_iter()
                .map(|name| {
                    IrNode::DropTable(DropTable {
                        name,
                        if_exists: drop.missing_ok,
                        cascade: drop.behavior() == pg_query::protobuf::DropBehavior::DropCascade,
                    })
                })
                .collect()
        }
        pg_query::protobuf::ObjectType::ObjectSchema => {
            let names = extract_schema_names_from_drop_objects(&drop.objects);
            if names.is_empty() {
                return vec![IrNode::Ignored {
                    raw_sql: raw_sql.to_string(),
                }];
            }
            names
                .into_iter()
                .map(|schema_name| {
                    IrNode::DropSchema(DropSchema {
                        schema_name,
                        if_exists: drop.missing_ok,
                        cascade: drop.behavior() == pg_query::protobuf::DropBehavior::DropCascade,
                    })
                })
                .collect()
        }
        _ => vec![IrNode::Ignored {
            raw_sql: raw_sql.to_string(),
        }],
    }
}

// ---------------------------------------------------------------------------
// TRUNCATE
// ---------------------------------------------------------------------------

/// Convert a pg_query `TruncateStmt` to one IR node per target table.
///
/// `TRUNCATE t1, t2, t3 CASCADE` produces three `TruncateTable` nodes,
/// all sharing the same `cascade` flag.
fn convert_truncate_stmt(trunc: &pg_query::protobuf::TruncateStmt) -> Vec<IrNode> {
    let cascade = trunc.behavior() == pg_query::protobuf::DropBehavior::DropCascade;

    trunc
        .relations
        .iter()
        .filter_map(|rel_node| {
            rel_node.node.as_ref().and_then(|n| match n {
                NodeEnum::RangeVar(rv) => Some(IrNode::TruncateTable(TruncateTable {
                    name: relation_to_qualified_name(Some(rv)),
                    cascade,
                })),
                _ => None,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// DML statements (INSERT, UPDATE, DELETE)
// ---------------------------------------------------------------------------

/// Convert an `InsertStmt` to `IrNode::InsertInto`.
fn convert_insert_stmt(insert: &pg_query::protobuf::InsertStmt) -> IrNode {
    let table_name = relation_to_qualified_name(insert.relation.as_ref());
    IrNode::InsertInto(InsertInto { table_name })
}

/// Convert an `UpdateStmt` to `IrNode::UpdateTable`.
fn convert_update_stmt(update: &pg_query::protobuf::UpdateStmt) -> IrNode {
    let table_name = relation_to_qualified_name(update.relation.as_ref());
    IrNode::UpdateTable(UpdateTable { table_name })
}

/// Convert a `DeleteStmt` to `IrNode::DeleteFrom`.
fn convert_delete_stmt(delete: &pg_query::protobuf::DeleteStmt) -> IrNode {
    let table_name = relation_to_qualified_name(delete.relation.as_ref());
    IrNode::DeleteFrom(DeleteFrom { table_name })
}

// ---------------------------------------------------------------------------
// CLUSTER
// ---------------------------------------------------------------------------

fn convert_cluster_stmt(cluster: &pg_query::protobuf::ClusterStmt) -> IrNode {
    let table = relation_to_qualified_name(cluster.relation.as_ref());
    let index = if cluster.indexname.is_empty() {
        None
    } else {
        Some(cluster.indexname.clone())
    };
    IrNode::Cluster(Cluster { table, index })
}

// ---------------------------------------------------------------------------
// DROP statement helpers
// ---------------------------------------------------------------------------

/// Extract ALL object names from `DropStmt.objects[]`.
///
/// For `DROP INDEX idx1, idx2`, returns `["idx1", "idx2"]`.
fn extract_all_names_from_drop_objects(objects: &[pg_query::protobuf::Node]) -> Vec<String> {
    objects
        .iter()
        .filter_map(|obj| {
            if let Some(NodeEnum::List(list)) = obj.node.as_ref() {
                // Take the last string item as the name
                list.items.iter().rev().find_map(|item| {
                    if let Some(NodeEnum::String(s)) = item.node.as_ref() {
                        Some(s.sval.clone())
                    } else {
                        None
                    }
                })
            } else {
                None
            }
        })
        .collect()
}

/// Extract schema names from `DropStmt.objects[]` for `DROP SCHEMA`.
///
/// Unlike tables/indexes (where objects are wrapped in `List` nodes), schema
/// names are stored as bare `String` nodes directly in the objects array.
fn extract_schema_names_from_drop_objects(objects: &[pg_query::protobuf::Node]) -> Vec<String> {
    objects
        .iter()
        .filter_map(|obj| {
            if let Some(NodeEnum::String(s)) = obj.node.as_ref() {
                Some(s.sval.clone())
            } else {
                None
            }
        })
        .collect()
}

/// Extract ALL qualified names from `DropStmt.objects[]` for multi-table DROP.
///
/// For `DROP TABLE foo, myschema.bar`, returns both names.
fn extract_all_qualified_names_from_drop_objects(
    objects: &[pg_query::protobuf::Node],
) -> Vec<QualifiedName> {
    objects
        .iter()
        .filter_map(|obj| {
            if let Some(NodeEnum::List(list)) = obj.node.as_ref() {
                let strings: Vec<String> = list
                    .items
                    .iter()
                    .filter_map(|item| match item.node.as_ref() {
                        Some(NodeEnum::String(s)) => Some(s.sval.clone()),
                        _ => None,
                    })
                    .collect();

                match strings.len() {
                    1 => Some(QualifiedName::unqualified(&strings[0])),
                    2 => Some(QualifiedName::qualified(&strings[0], &strings[1])),
                    _ if !strings.is_empty() => {
                        let name = strings.last().cloned().unwrap_or_default();
                        let schema = strings[strings.len() - 2].clone();
                        Some(QualifiedName::qualified(schema, name))
                    }
                    _ => None,
                }
            } else {
                None
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a pg_query `RangeVar` (relation reference) to a `QualifiedName`.
fn relation_to_qualified_name(rel: Option<&pg_query::protobuf::RangeVar>) -> QualifiedName {
    match rel {
        Some(r) => {
            if r.schemaname.is_empty() {
                QualifiedName::unqualified(&r.relname)
            } else {
                QualifiedName::qualified(&r.schemaname, &r.relname)
            }
        }
        None => QualifiedName::unqualified("unknown"),
    }
}

/// Map a `RangeVar`'s `relpersistence` field to `TablePersistence`.
///
/// In pg_query, the `relpersistence` field is a single character:
/// - `'t'` → Temporary
/// - `'u'` → Unlogged
/// - anything else (typically `'p'` or empty) → Permanent
fn relation_persistence(rel: Option<&pg_query::protobuf::RangeVar>) -> TablePersistence {
    match rel.map(|r| r.relpersistence.as_str()) {
        Some("u") => TablePersistence::Unlogged,
        Some("t") => TablePersistence::Temporary,
        _ => TablePersistence::Permanent,
    }
}

/// Extract a list of string values from pg_query `Node` lists.
///
/// Used for extracting column names from `keys[]`, `fk_attrs[]`, `pk_attrs[]`.
fn extract_string_list(nodes: &[pg_query::protobuf::Node]) -> Vec<String> {
    nodes
        .iter()
        .filter_map(|n| match n.node.as_ref() {
            Some(NodeEnum::String(s)) => Some(s.sval.clone()),
            _ => None,
        })
        .collect()
}

/// Extract column references from a pg_query expression node.
///
/// Recursively walks the AST collecting `ColumnRef` field names. For `ColumnRef`
/// nodes, the last `String` field is taken as the column name (earlier fields
/// are schema/table qualifiers). Constant nodes are skipped.
///
/// Covers: `ColumnRef`, `FuncCall`, `TypeCast`, `A_Expr`, `BoolExpr`,
/// `CaseExpr`, `CaseWhen`, `CoalesceExpr`, `NullTest`, `MinMaxExpr`.
fn extract_column_refs(node: &pg_query::protobuf::Node) -> Vec<String> {
    let mut refs = Vec::new();
    walk_node_for_column_refs(node, &mut refs);
    refs.sort();
    refs.dedup();
    refs
}

/// Recursive helper for [`extract_column_refs`].
fn walk_node_for_column_refs(node: &pg_query::protobuf::Node, refs: &mut Vec<String>) {
    let Some(inner) = &node.node else {
        return;
    };

    match inner {
        NodeEnum::ColumnRef(cr) => {
            // Take the last String field as the column name.
            // Earlier fields are schema/table qualifiers.
            if let Some(last) = cr.fields.last()
                && let Some(NodeEnum::String(s)) = &last.node
            {
                refs.push(s.sval.clone());
            }
        }
        NodeEnum::FuncCall(fc) => {
            for arg in &fc.args {
                walk_node_for_column_refs(arg, refs);
            }
        }
        NodeEnum::TypeCast(tc) => {
            if let Some(arg) = &tc.arg {
                walk_node_for_column_refs(arg, refs);
            }
        }
        NodeEnum::AExpr(expr) => {
            if let Some(lexpr) = &expr.lexpr {
                walk_node_for_column_refs(lexpr, refs);
            }
            if let Some(rexpr) = &expr.rexpr {
                walk_node_for_column_refs(rexpr, refs);
            }
        }
        NodeEnum::BoolExpr(be) => {
            for arg in &be.args {
                walk_node_for_column_refs(arg, refs);
            }
        }
        NodeEnum::CaseExpr(ce) => {
            if let Some(arg) = &ce.arg {
                walk_node_for_column_refs(arg, refs);
            }
            for when in &ce.args {
                walk_node_for_column_refs(when, refs);
            }
            if let Some(def) = &ce.defresult {
                walk_node_for_column_refs(def, refs);
            }
        }
        NodeEnum::CaseWhen(cw) => {
            if let Some(expr) = &cw.expr {
                walk_node_for_column_refs(expr, refs);
            }
            if let Some(result) = &cw.result {
                walk_node_for_column_refs(result, refs);
            }
        }
        NodeEnum::CoalesceExpr(ce) => {
            for arg in &ce.args {
                walk_node_for_column_refs(arg, refs);
            }
        }
        NodeEnum::NullTest(nt) => {
            if let Some(arg) = &nt.arg {
                walk_node_for_column_refs(arg, refs);
            }
        }
        NodeEnum::MinMaxExpr(mm) => {
            for arg in &mm.args {
                walk_node_for_column_refs(arg, refs);
            }
        }
        // Constants and other nodes — no column references.
        _ => {}
    }
}

/// Deparse a pg_query node back to SQL text using pg_query's deparser.
///
/// Falls back to a debug representation if deparsing fails.
///
/// Instead of hardcoding a PostgreSQL version number (which causes a SIGABRT when
/// the linked libpg_query has a different `PG_VERSION_NUM`), we parse a trivial
/// `SELECT NULL` statement to obtain a `ParseResult` with the correct version,
/// then splice our target node into its AST before deparsing.
fn deparse_node(node: &pg_query::protobuf::Node) -> String {
    // Parse a trivial SELECT to get a ParseResult with the correct version
    let mut parse_result = match pg_query::parse("SELECT NULL") {
        Ok(pr) => pr,
        Err(_) => return format!("{:?}", node.node),
    };

    // Replace the target list's value with our node
    if let Some(stmt) = parse_result.protobuf.stmts.first_mut()
        && let Some(ref mut stmt_node) = stmt.stmt
        && let Some(NodeEnum::SelectStmt(ref mut select)) = stmt_node.node
        && let Some(first_target) = select.target_list.first_mut()
        && let Some(NodeEnum::ResTarget(ref mut res)) = first_target.node
    {
        res.val = Some(Box::new(node.clone()));
    }

    match pg_query::deparse(&parse_result.protobuf) {
        Ok(sql) => {
            // Strip the "SELECT " prefix
            sql.strip_prefix("SELECT ").unwrap_or(&sql).to_string()
        }
        Err(_) => format!("{:?}", node.node),
    }
}

/// Try to extract a table name hint from raw SQL that failed to parse.
///
/// This is a best-effort heuristic used to mark tables as incomplete in the catalog.
fn extract_table_hint_from_raw(sql: &str) -> Option<String> {
    let upper = sql.to_uppercase();

    // Try to find "ALTER TABLE <name>" pattern
    let kw = "ALTER TABLE";
    if let Some(pos) = upper.find(kw) {
        let rest = &sql[pos + kw.len()..];
        return extract_first_identifier(rest);
    }

    // Try to find "CREATE TABLE <name>" pattern
    let kw = "CREATE TABLE";
    if let Some(pos) = upper.find(kw) {
        let rest = &sql[pos + kw.len()..];
        return extract_first_identifier(rest);
    }

    None
}

/// Extract the first SQL identifier from a string, skipping whitespace and keywords.
fn extract_first_identifier(s: &str) -> Option<String> {
    let trimmed = s.trim();

    // Skip optional keywords before the identifier
    let trimmed = {
        let upper = trimmed.to_uppercase();
        let skip_len = if upper.starts_with("IF NOT EXISTS") {
            "IF NOT EXISTS".len()
        } else if upper.starts_with("IF EXISTS") {
            "IF EXISTS".len()
        } else if upper.starts_with("ONLY") {
            "ONLY".len()
        } else {
            0
        };
        trimmed[skip_len..].trim()
    };

    // Take characters that could be part of an identifier (letters, digits, _, .)
    let ident: String = trimmed
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.' || *c == '"')
        .collect();

    if ident.is_empty() {
        None
    } else {
        // Strip quotes and return the full identifier (may be schema-qualified)
        let cleaned = ident.replace('"', "");
        Some(cleaned)
    }
}
