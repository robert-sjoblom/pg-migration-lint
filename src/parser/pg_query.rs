//! pg_query AST to IR conversion
//!
//! This module converts the pg_query crate's PostgreSQL AST into the simplified
//! IR layer used by the rule engine. It handles type canonicalization, constraint
//! normalization, and source location tracking.

use crate::parser::ir::{
    AlterTable, AlterTableAction, Cluster, ColumnDef, CreateIndex, CreateTable, DefaultExpr,
    DeleteFrom, DropIndex, DropTable, IndexColumn, InsertInto, IrNode, Located, QualifiedName,
    SourceSpan, TableConstraint, TablePersistence, TruncateTable, TypeName, UpdateTable,
};
use pg_query::NodeEnum;

/// Sentinel type name used when the actual type cannot be determined.
const UNKNOWN_TYPE: &str = "unknown";

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
        NodeEnum::AlterTableStmt(alter) => vec![convert_alter_table(alter, raw_sql)],
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

    IrNode::CreateTable(CreateTable {
        name,
        columns,
        constraints,
        persistence,
        if_not_exists: create.if_not_exists,
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
        other => vec![AlterTableAction::Other {
            description: format!("{:?}", other),
        }],
    }
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
                if elem.name.is_empty() {
                    None
                } else {
                    Some(IndexColumn {
                        name: elem.name.clone(),
                    })
                }
            }
            _ => None,
        })
        .collect();

    IrNode::CreateIndex(CreateIndex {
        index_name,
        table_name,
        columns,
        unique: idx.unique,
        concurrent: idx.concurrent,
        if_not_exists: idx.if_not_exists,
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

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // byte_offset_to_line
    // -----------------------------------------------------------------------

    #[test]
    fn test_byte_offset_to_line_first_line() {
        assert_eq!(byte_offset_to_line("hello\nworld", 0), 1);
        assert_eq!(byte_offset_to_line("hello\nworld", 3), 1);
    }

    #[test]
    fn test_byte_offset_to_line_second_line() {
        assert_eq!(byte_offset_to_line("hello\nworld", 6), 2);
        assert_eq!(byte_offset_to_line("hello\nworld", 10), 2);
    }

    #[test]
    fn test_byte_offset_to_line_at_newline() {
        // Offset at the newline character itself: still line 1
        assert_eq!(byte_offset_to_line("hello\nworld", 5), 1);
    }

    #[test]
    fn test_byte_offset_to_line_multi_line() {
        let source = "line1\nline2\nline3\nline4";
        assert_eq!(byte_offset_to_line(source, 0), 1);
        assert_eq!(byte_offset_to_line(source, 6), 2);
        assert_eq!(byte_offset_to_line(source, 12), 3);
        assert_eq!(byte_offset_to_line(source, 18), 4);
    }

    #[test]
    fn test_byte_offset_to_line_beyond_end() {
        // Should clamp to source length
        assert_eq!(byte_offset_to_line("hello", 999), 1);
    }

    // -----------------------------------------------------------------------
    // parse_sql — smoke tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_create_table() {
        let sql = "CREATE TABLE orders (id integer PRIMARY KEY, status text NOT NULL);";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert_eq!(ct.name, QualifiedName::unqualified("orders"));
                assert_eq!(ct.columns.len(), 2);
                assert_eq!(ct.columns[0].name, "id");
                assert_eq!(ct.columns[0].type_name.name, "int4");
                assert!(ct.columns[0].is_inline_pk);
                assert!(!ct.columns[0].nullable);
                assert_eq!(ct.columns[1].name, "status");
                assert_eq!(ct.columns[1].type_name.name, "text");
                assert!(!ct.columns[1].nullable);
                assert!(!ct.if_not_exists);
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_create_table_with_schema() {
        let sql = "CREATE TABLE myschema.orders (id int);";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert_eq!(ct.name, QualifiedName::qualified("myschema", "orders"));
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_serial_type() {
        let sql = "CREATE TABLE t (id serial);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert_eq!(ct.columns[0].type_name.name, "int4");
                assert!(
                    matches!(ct.columns[0].default_expr, Some(DefaultExpr::FunctionCall { ref name, .. }) if name == "nextval")
                );
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_bigserial_type() {
        let sql = "CREATE TABLE t (id bigserial);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert_eq!(ct.columns[0].type_name.name, "int8");
                assert!(ct.columns[0].default_expr.is_some());
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_smallserial_type() {
        let sql = "CREATE TABLE t (id smallserial);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert_eq!(ct.columns[0].type_name.name, "int2");
                assert!(
                    matches!(ct.columns[0].default_expr, Some(DefaultExpr::FunctionCall { ref name, .. }) if name == "nextval")
                );
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_varchar_with_modifier() {
        let sql = "CREATE TABLE t (name varchar(100));";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert_eq!(ct.columns[0].type_name.name, "varchar");
                assert_eq!(ct.columns[0].type_name.modifiers, vec![100]);
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_numeric_with_modifiers() {
        let sql = "CREATE TABLE t (price numeric(10,2));";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert_eq!(ct.columns[0].type_name.name, "numeric");
                assert_eq!(ct.columns[0].type_name.modifiers, vec![10, 2]);
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_type_canonicalization() {
        // All integer aliases should map to int4
        for sql in &[
            "CREATE TABLE t (col int);",
            "CREATE TABLE t (col integer);",
            "CREATE TABLE t (col int4);",
        ] {
            let nodes = parse_sql(sql);
            match &nodes[0].node {
                IrNode::CreateTable(ct) => {
                    assert_eq!(ct.columns[0].type_name.name, "int4", "Failed for: {}", sql);
                }
                other => panic!("Expected CreateTable for {}, got: {:?}", sql, other),
            }
        }
    }

    #[test]
    fn test_parse_default_literal() {
        let sql = "CREATE TABLE t (col int DEFAULT 0);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert_eq!(
                    ct.columns[0].default_expr,
                    Some(DefaultExpr::Literal("0".to_string()))
                );
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_default_function_call() {
        let sql = "CREATE TABLE t (col timestamp DEFAULT now());";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => match &ct.columns[0].default_expr {
                Some(DefaultExpr::FunctionCall { name, .. }) => {
                    assert_eq!(name, "now");
                }
                other => panic!("Expected FunctionCall default, got: {:?}", other),
            },
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_default_string_literal() {
        let sql = "CREATE TABLE t (col text DEFAULT 'active');";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert_eq!(
                    ct.columns[0].default_expr,
                    Some(DefaultExpr::Literal("active".to_string()))
                );
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Constraints
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_table_level_primary_key() {
        let sql = "CREATE TABLE t (id int, name text, PRIMARY KEY (id));";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert!(
                    ct.constraints.iter().any(|c| matches!(
                        c,
                        TableConstraint::PrimaryKey { columns, .. } if columns == &["id"]
                    )),
                    "Expected PrimaryKey constraint, got: {:?}",
                    ct.constraints
                );
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_inline_primary_key() {
        let sql = "CREATE TABLE t (id int PRIMARY KEY);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert!(ct.columns[0].is_inline_pk);
                assert!(ct.constraints.iter().any(|c| matches!(
                    c,
                    TableConstraint::PrimaryKey { columns, .. } if columns == &["id"]
                )),);
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_table_level_foreign_key() {
        let sql = "CREATE TABLE orders (customer_id int, FOREIGN KEY (customer_id) REFERENCES customers(id));";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert!(
                    ct.constraints.iter().any(|c| matches!(
                        c,
                        TableConstraint::ForeignKey {
                            columns, ref_table, ref_columns, ..
                        } if columns == &["customer_id"]
                            && ref_table == &QualifiedName::unqualified("customers")
                            && ref_columns == &["id"]
                    )),
                    "Expected ForeignKey constraint, got: {:?}",
                    ct.constraints
                );
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_inline_foreign_key() {
        let sql = "CREATE TABLE orders (customer_id int REFERENCES customers(id));";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert!(
                    ct.constraints.iter().any(|c| matches!(
                        c,
                        TableConstraint::ForeignKey {
                            columns, ref_table, ref_columns, ..
                        } if columns == &["customer_id"]
                            && ref_table == &QualifiedName::unqualified("customers")
                            && ref_columns == &["id"]
                    )),
                    "Expected ForeignKey constraint, got: {:?}",
                    ct.constraints
                );
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_unique_constraint() {
        let sql = "CREATE TABLE t (email text, UNIQUE (email));";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert!(ct.constraints.iter().any(|c| matches!(
                    c,
                    TableConstraint::Unique { columns, .. } if columns == &["email"]
                )),);
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // ALTER TABLE
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_alter_table_add_column() {
        let sql = "ALTER TABLE orders ADD COLUMN status text NOT NULL DEFAULT 'pending';";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::AlterTable(at) => {
                assert_eq!(at.name, QualifiedName::unqualified("orders"));
                assert_eq!(at.actions.len(), 1);
                match &at.actions[0] {
                    AlterTableAction::AddColumn(col) => {
                        assert_eq!(col.name, "status");
                        assert_eq!(col.type_name.name, "text");
                        assert!(!col.nullable);
                        assert_eq!(
                            col.default_expr,
                            Some(DefaultExpr::Literal("pending".to_string()))
                        );
                    }
                    other => panic!("Expected AddColumn, got: {:?}", other),
                }
            }
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_alter_table_drop_column() {
        let sql = "ALTER TABLE orders DROP COLUMN old_field;";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::AlterTable(at) => {
                assert_eq!(at.actions.len(), 1);
                match &at.actions[0] {
                    AlterTableAction::DropColumn { name } => {
                        assert_eq!(name, "old_field");
                    }
                    other => panic!("Expected DropColumn, got: {:?}", other),
                }
            }
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_alter_table_add_constraint() {
        let sql = "ALTER TABLE orders ADD CONSTRAINT fk_customer FOREIGN KEY (customer_id) REFERENCES customers(id);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::AlterTable(at) => {
                assert_eq!(at.actions.len(), 1);
                match &at.actions[0] {
                    AlterTableAction::AddConstraint(tc) => match tc {
                        TableConstraint::ForeignKey {
                            name,
                            columns,
                            ref_table,
                            ref_columns,
                            ..
                        } => {
                            assert_eq!(name.as_deref(), Some("fk_customer"));
                            assert_eq!(columns, &["customer_id"]);
                            assert_eq!(ref_table, &QualifiedName::unqualified("customers"));
                            assert_eq!(ref_columns, &["id"]);
                        }
                        other => panic!("Expected ForeignKey, got: {:?}", other),
                    },
                    other => panic!("Expected AddConstraint, got: {:?}", other),
                }
            }
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_alter_table_alter_column_type() {
        let sql = "ALTER TABLE orders ALTER COLUMN status TYPE varchar(100);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::AlterTable(at) => {
                assert_eq!(at.actions.len(), 1);
                match &at.actions[0] {
                    AlterTableAction::AlterColumnType {
                        column_name,
                        new_type,
                        old_type,
                    } => {
                        assert_eq!(column_name, "status");
                        assert_eq!(new_type.name, "varchar");
                        assert_eq!(new_type.modifiers, vec![100]);
                        assert!(old_type.is_none());
                    }
                    other => panic!("Expected AlterColumnType, got: {:?}", other),
                }
            }
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_alter_table_set_not_null() {
        let sql = "ALTER TABLE orders ALTER COLUMN price SET NOT NULL;";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::AlterTable(at) => {
                assert_eq!(at.actions.len(), 1);
                match &at.actions[0] {
                    AlterTableAction::SetNotNull { column_name } => {
                        assert_eq!(column_name, "price");
                    }
                    other => panic!("Expected SetNotNull, got: {:?}", other),
                }
            }
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // CREATE INDEX
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_create_index() {
        let sql = "CREATE INDEX idx_status ON orders (status);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateIndex(ci) => {
                assert_eq!(ci.index_name, Some("idx_status".to_string()));
                assert_eq!(ci.table_name, QualifiedName::unqualified("orders"));
                assert_eq!(ci.columns.len(), 1);
                assert_eq!(ci.columns[0].name, "status");
                assert!(!ci.unique);
                assert!(!ci.concurrent);
                assert!(!ci.if_not_exists);
            }
            other => panic!("Expected CreateIndex, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_create_index_concurrently() {
        let sql = "CREATE INDEX CONCURRENTLY idx_status ON orders (status);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateIndex(ci) => {
                assert!(ci.concurrent);
                assert!(!ci.unique);
            }
            other => panic!("Expected CreateIndex, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_create_unique_index() {
        let sql = "CREATE UNIQUE INDEX idx_email ON users (email);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateIndex(ci) => {
                assert!(ci.unique);
                assert!(!ci.concurrent);
            }
            other => panic!("Expected CreateIndex, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_create_composite_index() {
        let sql = "CREATE INDEX idx_composite ON orders (customer_id, status);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateIndex(ci) => {
                assert_eq!(ci.columns.len(), 2);
                assert_eq!(ci.columns[0].name, "customer_id");
                assert_eq!(ci.columns[1].name, "status");
            }
            other => panic!("Expected CreateIndex, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // DROP INDEX / DROP TABLE
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_drop_index() {
        let sql = "DROP INDEX idx_status;";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::DropIndex(di) => {
                assert_eq!(di.index_name, "idx_status");
                assert!(!di.concurrent);
            }
            other => panic!("Expected DropIndex, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_drop_index_concurrently() {
        let sql = "DROP INDEX CONCURRENTLY idx_status;";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::DropIndex(di) => {
                assert_eq!(di.index_name, "idx_status");
                assert!(di.concurrent);
            }
            other => panic!("Expected DropIndex, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_drop_table() {
        let sql = "DROP TABLE orders;";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::DropTable(dt) => {
                assert_eq!(dt.name, QualifiedName::unqualified("orders"));
                assert!(!dt.cascade, "Plain DROP TABLE should not have cascade");
            }
            other => panic!("Expected DropTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_drop_table_with_schema() {
        let sql = "DROP TABLE myschema.orders;";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::DropTable(dt) => {
                assert_eq!(dt.name, QualifiedName::qualified("myschema", "orders"));
                assert!(!dt.cascade);
            }
            other => panic!("Expected DropTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_drop_table_cascade() {
        let sql = "DROP TABLE orders CASCADE;";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::DropTable(dt) => {
                assert_eq!(dt.name, QualifiedName::unqualified("orders"));
                assert!(dt.cascade, "DROP TABLE CASCADE should have cascade=true");
                assert!(!dt.if_exists);
            }
            other => panic!("Expected DropTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_drop_table_if_exists_cascade() {
        let sql = "DROP TABLE IF EXISTS orders CASCADE;";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::DropTable(dt) => {
                assert_eq!(dt.name, QualifiedName::unqualified("orders"));
                assert!(dt.cascade);
                assert!(dt.if_exists);
            }
            other => panic!("Expected DropTable, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // DO blocks and ignored statements
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_do_block_as_unparseable() {
        let sql = "DO $$ BEGIN RAISE NOTICE 'hello'; END $$;";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::Unparseable { .. } => {} // Expected
            other => panic!("Expected Unparseable for DO block, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_grant_as_ignored() {
        let sql = "GRANT SELECT ON orders TO readonly;";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::Ignored { .. } => {} // Expected
            other => panic!("Expected Ignored for GRANT, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_comment_on_as_ignored() {
        let sql = "COMMENT ON TABLE orders IS 'Order table';";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::Ignored { .. } => {} // Expected
            other => panic!("Expected Ignored for COMMENT ON, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Multi-statement parsing
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_multi_statement() {
        let sql = "CREATE TABLE foo (id int);\nCREATE INDEX idx ON foo (id);\nALTER TABLE foo ADD COLUMN name text;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 3);

        assert!(matches!(nodes[0].node, IrNode::CreateTable(_)));
        assert!(matches!(nodes[1].node, IrNode::CreateIndex(_)));
        assert!(matches!(nodes[2].node, IrNode::AlterTable(_)));

        // Check line numbers
        assert_eq!(nodes[0].span.start_line, 1);
        assert_eq!(nodes[1].span.start_line, 2);
        assert_eq!(nodes[2].span.start_line, 3);
    }

    #[test]
    fn test_parse_invalid_sql() {
        let sql = "THIS IS NOT VALID SQL AT ALL;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        assert!(matches!(nodes[0].node, IrNode::Unparseable { .. }));
    }

    // -----------------------------------------------------------------------
    // Source span accuracy
    // -----------------------------------------------------------------------

    #[test]
    fn test_source_span_single_statement() {
        let sql = "CREATE TABLE foo (id int);";
        let nodes = parse_sql(sql);
        assert_eq!(nodes[0].span.start_line, 1);
        assert_eq!(nodes[0].span.end_line, 1);
        assert_eq!(nodes[0].span.start_offset, 0);
    }

    #[test]
    fn test_source_span_multi_line_statement() {
        let sql = "CREATE TABLE foo (\n  id int,\n  name text\n);";
        let nodes = parse_sql(sql);
        assert_eq!(nodes[0].span.start_line, 1);
        assert_eq!(nodes[0].span.end_line, 4);
    }

    // -----------------------------------------------------------------------
    // Table hint extraction
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_table_hint_alter() {
        let hint = extract_table_hint_from_raw("ALTER TABLE orders ADD COLUMN x int;");
        assert_eq!(hint.as_deref(), Some("orders"));
    }

    #[test]
    fn test_extract_table_hint_create() {
        let hint = extract_table_hint_from_raw("CREATE TABLE IF NOT EXISTS orders (id int);");
        assert_eq!(hint.as_deref(), Some("orders"));
    }

    #[test]
    fn test_extract_table_hint_alter_lowercase() {
        let hint = extract_table_hint_from_raw("alter table orders add column x int;");
        assert_eq!(hint.as_deref(), Some("orders"));
    }

    #[test]
    fn test_extract_table_hint_create_if_exists() {
        let hint = extract_table_hint_from_raw("CREATE TABLE IF EXISTS orders (id int);");
        assert_eq!(hint.as_deref(), Some("orders"));
    }

    #[test]
    fn test_extract_table_hint_alter_only() {
        let hint = extract_table_hint_from_raw("ALTER TABLE ONLY orders ADD COLUMN x int;");
        assert_eq!(hint.as_deref(), Some("orders"));
    }

    #[test]
    fn test_extract_table_hint_schema_qualified() {
        let hint = extract_table_hint_from_raw("ALTER TABLE public.orders ADD COLUMN x int;");
        assert_eq!(hint.as_deref(), Some("public.orders"));
    }

    #[test]
    fn test_extract_table_hint_none() {
        let hint = extract_table_hint_from_raw("DO $$ BEGIN END $$;");
        assert!(hint.is_none());
    }

    // -----------------------------------------------------------------------
    // Nullable inference from constraints
    // -----------------------------------------------------------------------

    #[test]
    fn test_nullable_column_default() {
        let sql = "CREATE TABLE t (name text);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert!(
                    ct.columns[0].nullable,
                    "Column without NOT NULL should be nullable"
                );
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_not_null_column() {
        let sql = "CREATE TABLE t (name text NOT NULL);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert!(
                    !ct.columns[0].nullable,
                    "Column with NOT NULL should not be nullable"
                );
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_primary_key_implies_not_null() {
        let sql = "CREATE TABLE t (id int PRIMARY KEY);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert!(
                    !ct.columns[0].nullable,
                    "PRIMARY KEY column should not be nullable"
                );
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // check constraint
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_check_constraint() {
        let sql = "CREATE TABLE t (col int, CHECK (col > 0));";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert!(
                    ct.constraints
                        .iter()
                        .any(|c| matches!(c, TableConstraint::Check { .. })),
                    "Expected CHECK constraint, got: {:?}",
                    ct.constraints
                );
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // ALTER TABLE ADD COLUMN with serial
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_alter_add_column_serial() {
        let sql = "ALTER TABLE t ADD COLUMN seq_id serial;";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::AlterTable(at) => match &at.actions[0] {
                AlterTableAction::AddColumn(col) => {
                    assert_eq!(col.type_name.name, "int4");
                    assert!(matches!(
                        col.default_expr,
                        Some(DefaultExpr::FunctionCall { ref name, .. }) if name == "nextval"
                    ));
                }
                other => panic!("Expected AddColumn, got: {:?}", other),
            },
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Boolean default
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_boolean_default() {
        let sql = "CREATE TABLE t (active bool DEFAULT TRUE);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => match &ct.columns[0].default_expr {
                Some(DefaultExpr::Literal(v)) => {
                    assert_eq!(v, "true");
                }
                other => panic!("Expected Literal default, got: {:?}", other),
            },
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Named constraint via ALTER TABLE
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_alter_table_add_primary_key() {
        let sql = "ALTER TABLE t ADD PRIMARY KEY (id);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::AlterTable(at) => match &at.actions[0] {
                AlterTableAction::AddConstraint(TableConstraint::PrimaryKey {
                    columns,
                    using_index,
                }) => {
                    assert_eq!(columns, &["id"]);
                    assert_eq!(*using_index, None);
                }
                other => panic!("Expected AddConstraint PrimaryKey, got: {:?}", other),
            },
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // USING INDEX
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_add_pk_using_index() {
        let sql = "ALTER TABLE t ADD PRIMARY KEY USING INDEX idx_foo;";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::AlterTable(at) => match &at.actions[0] {
                AlterTableAction::AddConstraint(TableConstraint::PrimaryKey {
                    columns,
                    using_index,
                }) => {
                    assert!(
                        columns.is_empty(),
                        "columns should be empty with USING INDEX"
                    );
                    assert_eq!(using_index.as_deref(), Some("idx_foo"));
                }
                other => panic!("Expected AddConstraint PrimaryKey, got: {:?}", other),
            },
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_add_unique_using_index() {
        let sql = "ALTER TABLE t ADD CONSTRAINT uq_email UNIQUE USING INDEX idx_email;";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::AlterTable(at) => match &at.actions[0] {
                AlterTableAction::AddConstraint(TableConstraint::Unique {
                    name,
                    columns,
                    using_index,
                }) => {
                    assert_eq!(name.as_deref(), Some("uq_email"));
                    assert!(
                        columns.is_empty(),
                        "columns should be empty with USING INDEX"
                    );
                    assert_eq!(using_index.as_deref(), Some("idx_email"));
                }
                other => panic!("Expected AddConstraint Unique, got: {:?}", other),
            },
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_add_pk_without_using_index() {
        let sql = "ALTER TABLE t ADD PRIMARY KEY (id);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::AlterTable(at) => match &at.actions[0] {
                AlterTableAction::AddConstraint(TableConstraint::PrimaryKey {
                    columns,
                    using_index,
                }) => {
                    assert_eq!(columns, &["id"]);
                    assert_eq!(*using_index, None);
                }
                other => panic!("Expected AddConstraint PrimaryKey, got: {:?}", other),
            },
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_inline_pk_no_using_index() {
        let sql = "CREATE TABLE t (id int PRIMARY KEY);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                let pk = ct
                    .constraints
                    .iter()
                    .find(|c| matches!(c, TableConstraint::PrimaryKey { .. }))
                    .expect("should have PK constraint");
                match pk {
                    TableConstraint::PrimaryKey {
                        columns,
                        using_index,
                    } => {
                        assert_eq!(columns, &["id"]);
                        assert_eq!(*using_index, None);
                    }
                    _ => unreachable!(),
                }
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_inline_unique_no_using_index() {
        let sql = "CREATE TABLE t (email text UNIQUE);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                let uq = ct
                    .constraints
                    .iter()
                    .find(|c| matches!(c, TableConstraint::Unique { .. }))
                    .expect("should have UNIQUE constraint");
                match uq {
                    TableConstraint::Unique { using_index, .. } => {
                        assert_eq!(*using_index, None);
                    }
                    _ => unreachable!(),
                }
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // NOT VALID constraints
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_add_fk_not_valid() {
        let sql = "ALTER TABLE orders ADD CONSTRAINT fk_customer FOREIGN KEY (customer_id) REFERENCES customers(id) NOT VALID;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::AlterTable(at) => {
                assert_eq!(at.actions.len(), 1);
                match &at.actions[0] {
                    AlterTableAction::AddConstraint(TableConstraint::ForeignKey {
                        name,
                        columns,
                        ref_table,
                        ref_columns,
                        not_valid,
                    }) => {
                        assert_eq!(name.as_deref(), Some("fk_customer"));
                        assert_eq!(columns, &["customer_id"]);
                        assert_eq!(ref_table, &QualifiedName::unqualified("customers"));
                        assert_eq!(ref_columns, &["id"]);
                        assert!(*not_valid, "Expected not_valid to be true");
                    }
                    other => panic!("Expected AddConstraint ForeignKey, got: {:?}", other),
                }
            }
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_add_fk_without_not_valid() {
        let sql = "ALTER TABLE orders ADD CONSTRAINT fk_customer FOREIGN KEY (customer_id) REFERENCES customers(id);";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::AlterTable(at) => {
                assert_eq!(at.actions.len(), 1);
                match &at.actions[0] {
                    AlterTableAction::AddConstraint(TableConstraint::ForeignKey {
                        not_valid,
                        ..
                    }) => {
                        assert!(!*not_valid, "Expected not_valid to be false");
                    }
                    other => panic!("Expected AddConstraint ForeignKey, got: {:?}", other),
                }
            }
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_add_check_not_valid() {
        let sql = "ALTER TABLE orders ADD CONSTRAINT chk_amount CHECK (amount > 0) NOT VALID;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::AlterTable(at) => {
                assert_eq!(at.actions.len(), 1);
                match &at.actions[0] {
                    AlterTableAction::AddConstraint(TableConstraint::Check {
                        name,
                        not_valid,
                        ..
                    }) => {
                        assert_eq!(name.as_deref(), Some("chk_amount"));
                        assert!(*not_valid, "Expected not_valid to be true");
                    }
                    other => panic!("Expected AddConstraint Check, got: {:?}", other),
                }
            }
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_add_check_without_not_valid() {
        let sql = "ALTER TABLE orders ADD CONSTRAINT chk_amount CHECK (amount > 0);";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::AlterTable(at) => {
                assert_eq!(at.actions.len(), 1);
                match &at.actions[0] {
                    AlterTableAction::AddConstraint(TableConstraint::Check {
                        not_valid, ..
                    }) => {
                        assert!(!*not_valid, "Expected not_valid to be false");
                    }
                    other => panic!("Expected AddConstraint Check, got: {:?}", other),
                }
            }
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // RENAME TABLE / RENAME COLUMN / RENAME INDEX
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_rename_table() {
        let sql = "ALTER TABLE orders RENAME TO orders_v2;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::RenameTable { name, new_name } => {
                assert_eq!(name.name, "orders");
                assert_eq!(new_name, "orders_v2");
            }
            other => panic!("Expected RenameTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_rename_column() {
        let sql = "ALTER TABLE orders RENAME COLUMN status TO order_status;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::RenameColumn {
                table,
                old_name,
                new_name,
            } => {
                assert_eq!(table.name, "orders");
                assert_eq!(old_name, "status");
                assert_eq!(new_name, "order_status");
            }
            other => panic!("Expected RenameColumn, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_rename_index_ignored() {
        let sql = "ALTER INDEX idx_foo RENAME TO idx_bar;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::Ignored { .. } => {} // Expected
            other => panic!("Expected Ignored for ALTER INDEX RENAME, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Schema-qualified DROP INDEX
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_drop_index_schema_qualified() {
        let sql = "DROP INDEX myschema.idx_status;";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::DropIndex(di) => {
                assert_eq!(di.index_name, "idx_status");
                assert!(!di.concurrent);
            }
            other => panic!("Expected DropIndex, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Temp table filtering
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_create_temp_table() {
        let sql = "CREATE TEMP TABLE scratch (id int);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert_eq!(ct.name.name, "scratch");
                assert_eq!(
                    ct.persistence,
                    TablePersistence::Temporary,
                    "TEMP table should have Temporary persistence"
                );
                assert_eq!(ct.columns.len(), 1);
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_create_temporary_table() {
        let sql = "CREATE TEMPORARY TABLE scratch (id int);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert_eq!(
                    ct.persistence,
                    TablePersistence::Temporary,
                    "TEMPORARY table should have Temporary persistence"
                );
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_regular_table_not_temporary() {
        let sql = "CREATE TABLE regular (id int);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert_eq!(
                    ct.persistence,
                    TablePersistence::Permanent,
                    "Regular table should have Permanent persistence"
                );
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_create_unlogged_table() {
        let sql = "CREATE UNLOGGED TABLE scratch (id int, payload text);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert_eq!(ct.name.name, "scratch");
                assert_eq!(
                    ct.persistence,
                    TablePersistence::Unlogged,
                    "UNLOGGED table should have Unlogged persistence"
                );
                assert_eq!(ct.columns.len(), 2);
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // IF NOT EXISTS
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_create_table_if_not_exists() {
        let sql = "CREATE TABLE IF NOT EXISTS orders (id int);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert_eq!(ct.name, QualifiedName::unqualified("orders"));
                assert!(ct.if_not_exists);
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_create_index_if_not_exists() {
        let sql = "CREATE INDEX IF NOT EXISTS idx_status ON orders (status);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateIndex(ci) => {
                assert_eq!(ci.index_name, Some("idx_status".to_string()));
                assert!(ci.if_not_exists);
            }
            other => panic!("Expected CreateIndex, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // TRUNCATE TABLE
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_truncate_table() {
        let sql = "TRUNCATE TABLE foo;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::TruncateTable(tt) => {
                assert_eq!(tt.name, QualifiedName::unqualified("foo"));
                assert!(!tt.cascade);
            }
            other => panic!("Expected TruncateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_truncate_table_cascade() {
        let sql = "TRUNCATE TABLE foo CASCADE;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::TruncateTable(tt) => {
                assert_eq!(tt.name, QualifiedName::unqualified("foo"));
                assert!(tt.cascade);
            }
            other => panic!("Expected TruncateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_truncate_multi_table() {
        let sql = "TRUNCATE TABLE t1, t2, t3 CASCADE;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 3);
        let names: Vec<&str> = nodes
            .iter()
            .map(|n| match &n.node {
                IrNode::TruncateTable(tt) => {
                    assert!(tt.cascade);
                    tt.name.name.as_str()
                }
                other => panic!("Expected TruncateTable, got: {:?}", other),
            })
            .collect();
        assert_eq!(names, vec!["t1", "t2", "t3"]);
    }

    // -----------------------------------------------------------------------
    // Multi-table DROP TABLE / DROP INDEX (regression tests)
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_drop_table_multi() {
        let sql = "DROP TABLE t1, t2;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 2);
        match &nodes[0].node {
            IrNode::DropTable(dt) => {
                assert_eq!(dt.name, QualifiedName::unqualified("t1"));
            }
            other => panic!("Expected DropTable, got: {:?}", other),
        }
        match &nodes[1].node {
            IrNode::DropTable(dt) => {
                assert_eq!(dt.name, QualifiedName::unqualified("t2"));
            }
            other => panic!("Expected DropTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_drop_index_multi() {
        let sql = "DROP INDEX idx1, idx2;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 2);
        match &nodes[0].node {
            IrNode::DropIndex(di) => {
                assert_eq!(di.index_name, "idx1");
            }
            other => panic!("Expected DropIndex, got: {:?}", other),
        }
        match &nodes[1].node {
            IrNode::DropIndex(di) => {
                assert_eq!(di.index_name, "idx2");
            }
            other => panic!("Expected DropIndex, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // convert_drop_stmt — catch-all arm (non-table, non-index DROP)
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_drop_view_ignored() {
        let sql = "DROP VIEW my_view;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::Ignored { .. } => {} // Expected — DROP VIEW hits the catch-all arm
            other => panic!("Expected Ignored for DROP VIEW, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_drop_sequence_ignored() {
        let sql = "DROP SEQUENCE my_seq;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::Ignored { .. } => {} // Expected — DROP SEQUENCE hits the catch-all arm
            other => panic!("Expected Ignored for DROP SEQUENCE, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_drop_type_ignored() {
        let sql = "DROP TYPE my_type;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::Ignored { .. } => {} // Expected — DROP TYPE hits the catch-all arm
            other => panic!("Expected Ignored for DROP TYPE, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // TRUNCATE with schema-qualified name
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_truncate_schema_qualified() {
        let sql = "TRUNCATE TABLE myschema.foo;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::TruncateTable(tt) => {
                assert_eq!(tt.name, QualifiedName::qualified("myschema", "foo"));
                assert!(!tt.cascade);
            }
            other => panic!("Expected TruncateTable, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Single-target DROP TABLE / DROP INDEX variations
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_drop_table_single_no_cascade_no_if_exists() {
        let sql = "DROP TABLE foo;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::DropTable(dt) => {
                assert_eq!(dt.name, QualifiedName::unqualified("foo"));
                assert!(!dt.cascade);
                assert!(!dt.if_exists);
            }
            other => panic!("Expected DropTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_drop_table_if_exists() {
        let sql = "DROP TABLE IF EXISTS foo;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::DropTable(dt) => {
                assert_eq!(dt.name, QualifiedName::unqualified("foo"));
                assert!(!dt.cascade);
                assert!(dt.if_exists);
            }
            other => panic!("Expected DropTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_drop_table_cascade_without_if_exists() {
        let sql = "DROP TABLE foo CASCADE;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::DropTable(dt) => {
                assert_eq!(dt.name, QualifiedName::unqualified("foo"));
                assert!(dt.cascade);
                assert!(!dt.if_exists);
            }
            other => panic!("Expected DropTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_drop_index_single() {
        let sql = "DROP INDEX foo;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::DropIndex(di) => {
                assert_eq!(di.index_name, "foo");
                assert!(!di.concurrent);
                assert!(!di.if_exists);
            }
            other => panic!("Expected DropIndex, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_drop_index_if_exists() {
        let sql = "DROP INDEX IF EXISTS foo;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::DropIndex(di) => {
                assert_eq!(di.index_name, "foo");
                assert!(!di.concurrent);
                assert!(di.if_exists);
            }
            other => panic!("Expected DropIndex, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_drop_index_concurrently_if_exists() {
        let sql = "DROP INDEX CONCURRENTLY IF EXISTS foo;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::DropIndex(di) => {
                assert_eq!(di.index_name, "foo");
                assert!(di.concurrent);
                assert!(di.if_exists);
            }
            other => panic!("Expected DropIndex, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Schema-qualified DROP TABLE / DROP INDEX
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_drop_table_schema_qualified() {
        let sql = "DROP TABLE myschema.foo;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::DropTable(dt) => {
                assert_eq!(dt.name, QualifiedName::qualified("myschema", "foo"));
                assert!(!dt.cascade);
                assert!(!dt.if_exists);
            }
            other => panic!("Expected DropTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_drop_index_schema_qualified_with_flags() {
        let sql = "DROP INDEX CONCURRENTLY IF EXISTS myschema.idx_foo;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::DropIndex(di) => {
                // extract_all_names_from_drop_objects takes last string, which is the index name
                assert_eq!(di.index_name, "idx_foo");
                assert!(di.concurrent);
                assert!(di.if_exists);
            }
            other => panic!("Expected DropIndex, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // extract_all_qualified_names_from_drop_objects — 3+ component names
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_drop_table_three_part_name() {
        // In PostgreSQL, `catalog.schema.table` is a valid 3-part name.
        // pg_query parses this and produces 3 string components.
        // This exercises the `_ if !strings.is_empty()` arm (line ~777).
        let sql = "DROP TABLE mycat.myschema.foo;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::DropTable(dt) => {
                // The 3+ branch takes last two: schema = strings[len-2], name = strings[last]
                assert_eq!(dt.name, QualifiedName::qualified("myschema", "foo"));
                assert!(!dt.cascade);
                assert!(!dt.if_exists);
            }
            other => panic!("Expected DropTable, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Multi-target DROP TABLE with mixed qualified names
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_drop_table_multi_mixed_qualified() {
        let sql = "DROP TABLE foo, myschema.bar;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 2);
        match &nodes[0].node {
            IrNode::DropTable(dt) => {
                assert_eq!(dt.name, QualifiedName::unqualified("foo"));
            }
            other => panic!("Expected DropTable for foo, got: {:?}", other),
        }
        match &nodes[1].node {
            IrNode::DropTable(dt) => {
                assert_eq!(dt.name, QualifiedName::qualified("myschema", "bar"));
            }
            other => panic!("Expected DropTable for bar, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_drop_table_multi_cascade_if_exists() {
        let sql = "DROP TABLE IF EXISTS t1, t2 CASCADE;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 2);
        for (i, expected_name) in ["t1", "t2"].iter().enumerate() {
            match &nodes[i].node {
                IrNode::DropTable(dt) => {
                    assert_eq!(dt.name, QualifiedName::unqualified(*expected_name));
                    assert!(dt.cascade, "Expected cascade for {}", expected_name);
                    assert!(dt.if_exists, "Expected if_exists for {}", expected_name);
                }
                other => panic!("Expected DropTable for {}, got: {:?}", expected_name, other),
            }
        }
    }

    // -----------------------------------------------------------------------
    // DROP INDEX — multi-target with schema-qualified names
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_drop_index_multi_schema_qualified() {
        let sql = "DROP INDEX myschema.idx1, myschema.idx2;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 2);
        match &nodes[0].node {
            IrNode::DropIndex(di) => {
                assert_eq!(di.index_name, "idx1");
            }
            other => panic!("Expected DropIndex for idx1, got: {:?}", other),
        }
        match &nodes[1].node {
            IrNode::DropIndex(di) => {
                assert_eq!(di.index_name, "idx2");
            }
            other => panic!("Expected DropIndex for idx2, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // TRUNCATE without TABLE keyword (bare TRUNCATE)
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_truncate_bare() {
        // "TRUNCATE foo" is valid SQL without the optional TABLE keyword
        let sql = "TRUNCATE foo;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::TruncateTable(tt) => {
                assert_eq!(tt.name, QualifiedName::unqualified("foo"));
                assert!(!tt.cascade);
            }
            other => panic!("Expected TruncateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_truncate_multi_schema_qualified() {
        let sql = "TRUNCATE TABLE myschema.t1, public.t2 CASCADE;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 2);
        match &nodes[0].node {
            IrNode::TruncateTable(tt) => {
                assert_eq!(tt.name, QualifiedName::qualified("myschema", "t1"));
                assert!(tt.cascade);
            }
            other => panic!("Expected TruncateTable, got: {:?}", other),
        }
        match &nodes[1].node {
            IrNode::TruncateTable(tt) => {
                assert_eq!(tt.name, QualifiedName::qualified("public", "t2"));
                assert!(tt.cascade);
            }
            other => panic!("Expected TruncateTable, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Additional convert_node catch-all tests (IrNode::Ignored paths)
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_create_view_ignored() {
        let sql = "CREATE VIEW v AS SELECT 1;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        assert!(matches!(nodes[0].node, IrNode::Ignored { .. }));
    }

    #[test]
    fn test_parse_create_function_ignored() {
        let sql = "CREATE FUNCTION add(a int, b int) RETURNS int AS 'SELECT a + b' LANGUAGE sql;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        assert!(matches!(nodes[0].node, IrNode::Ignored { .. }));
    }

    #[test]
    fn test_parse_create_sequence_ignored() {
        let sql = "CREATE SEQUENCE order_seq START 1;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        assert!(matches!(nodes[0].node, IrNode::Ignored { .. }));
    }

    #[test]
    fn test_parse_create_extension_ignored() {
        let sql = "CREATE EXTENSION IF NOT EXISTS pgcrypto;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        assert!(matches!(nodes[0].node, IrNode::Ignored { .. }));
    }

    #[test]
    fn test_parse_create_type_ignored() {
        let sql = "CREATE TYPE mood AS ENUM ('sad', 'ok', 'happy');";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        assert!(matches!(nodes[0].node, IrNode::Ignored { .. }));
    }

    #[test]
    fn test_parse_insert() {
        let sql = "INSERT INTO foo (id) VALUES (1);";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::InsertInto(ii) => {
                assert_eq!(ii.table_name.name, "foo");
            }
            other => panic!("Expected InsertInto, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_insert_schema_qualified() {
        let sql = "INSERT INTO myschema.foo (id) VALUES (1);";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::InsertInto(ii) => {
                assert_eq!(ii.table_name, QualifiedName::qualified("myschema", "foo"));
            }
            other => panic!("Expected InsertInto, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_update() {
        let sql = "UPDATE foo SET bar = 1 WHERE id = 2;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::UpdateTable(ut) => {
                assert_eq!(ut.table_name.name, "foo");
            }
            other => panic!("Expected UpdateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_delete() {
        let sql = "DELETE FROM foo WHERE id = 1;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::DeleteFrom(df) => {
                assert_eq!(df.table_name.name, "foo");
            }
            other => panic!("Expected DeleteFrom, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_select_ignored() {
        let sql = "SELECT * FROM foo;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        assert!(matches!(nodes[0].node, IrNode::Ignored { .. }));
    }

    #[test]
    fn test_parse_set_ignored() {
        let sql = "SET search_path TO myschema;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        assert!(matches!(nodes[0].node, IrNode::Ignored { .. }));
    }

    #[test]
    fn test_parse_revoke_ignored() {
        let sql = "REVOKE SELECT ON orders FROM readonly;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        assert!(matches!(nodes[0].node, IrNode::Ignored { .. }));
    }

    #[test]
    fn test_parse_create_trigger_ignored() {
        let sql = "CREATE TRIGGER trg BEFORE INSERT ON foo FOR EACH ROW EXECUTE FUNCTION bar();";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        assert!(matches!(nodes[0].node, IrNode::Ignored { .. }));
    }

    // -----------------------------------------------------------------------
    // ALTER TABLE — Other actions and empty-action edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_alter_table_enable_trigger_as_other() {
        // ALTER TABLE with an unrecognized subtype produces AlterTable with Other actions, not Ignored
        let sql = "ALTER TABLE foo ENABLE TRIGGER ALL;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::AlterTable(at) => {
                assert_eq!(at.name.name, "foo");
                assert_eq!(at.actions.len(), 1);
                assert!(matches!(at.actions[0], AlterTableAction::Other { .. }));
            }
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_alter_table_owner_as_other() {
        let sql = "ALTER TABLE foo OWNER TO new_owner;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        // OWNER TO is parsed as AlterTableStmt by pg_query but maps to Other action
        match &nodes[0].node {
            IrNode::AlterTable(at) => {
                assert_eq!(at.actions.len(), 1);
                assert!(matches!(at.actions[0], AlterTableAction::Other { .. }));
            }
            // Some pg_query versions may route OWNER TO elsewhere, accept Ignored too
            IrNode::Ignored { .. } => {}
            other => panic!("Expected AlterTable or Ignored, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_alter_table_drop_not_null() {
        let sql = "ALTER TABLE foo ALTER COLUMN bar DROP NOT NULL;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::AlterTable(at) => {
                assert_eq!(at.actions.len(), 1);
                match &at.actions[0] {
                    AlterTableAction::Other { description } => {
                        assert!(
                            description.contains("DROP NOT NULL"),
                            "got: {}",
                            description
                        );
                    }
                    other => panic!("Expected Other action, got: {:?}", other),
                }
            }
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Default expression coverage
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_default_float_literal() {
        let sql = "CREATE TABLE t (price numeric DEFAULT 9.99);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                // Float literals come through as Literal
                assert!(ct.columns[0].default_expr.is_some());
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_default_null() {
        let sql = "CREATE TABLE t (name text DEFAULT NULL);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert!(ct.columns[0].default_expr.is_some());
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_default_typecast() {
        let sql = "CREATE TABLE t (ts timestamptz DEFAULT '2024-01-01'::timestamptz);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                assert!(matches!(
                    ct.columns[0].default_expr,
                    Some(DefaultExpr::Other(_))
                ));
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_default_expression() {
        let sql = "CREATE TABLE t (computed int DEFAULT 1 + 2);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                // Arithmetic expressions should produce Other
                assert!(ct.columns[0].default_expr.is_some());
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // Other helper coverage
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_create_index_without_name() {
        // Anonymous indexes should have index_name == None
        let sql = "CREATE INDEX ON foo (bar);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateIndex(ci) => {
                assert!(ci.index_name.is_none());
                assert_eq!(ci.table_name.name, "foo");
            }
            other => panic!("Expected CreateIndex, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_rename_schema_ignored() {
        // ALTER ... RENAME that is not TABLE or COLUMN should be Ignored
        let sql = "ALTER SEQUENCE my_seq RENAME TO new_seq;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        // Sequence rename may go through RenameStmt catch-all
        assert!(
            matches!(nodes[0].node, IrNode::Ignored { .. }),
            "Expected Ignored for ALTER SEQUENCE RENAME, got: {:?}",
            nodes[0].node
        );
    }

    #[test]
    fn test_parse_drop_index_if_exists_coverage() {
        let sql = "DROP INDEX IF EXISTS idx_foo;";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::DropIndex(di) => {
                assert_eq!(di.index_name, "idx_foo");
                assert!(di.if_exists);
            }
            other => panic!("Expected DropIndex, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_alter_table_add_column_with_inline_fk() {
        let sql = "ALTER TABLE orders ADD COLUMN customer_id int REFERENCES customers(id);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::AlterTable(at) => {
                // Should produce AddColumn + AddConstraint (FK)
                assert!(
                    at.actions.len() >= 2,
                    "Expected at least 2 actions (AddColumn + FK), got {}",
                    at.actions.len()
                );
                assert!(matches!(at.actions[0], AlterTableAction::AddColumn(_)));
                assert!(matches!(
                    at.actions[1],
                    AlterTableAction::AddConstraint(TableConstraint::ForeignKey { .. })
                ));
            }
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_alter_table_add_column_with_inline_unique() {
        let sql = "ALTER TABLE t ADD COLUMN email text UNIQUE;";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::AlterTable(at) => {
                assert!(at.actions.len() >= 2);
                assert!(matches!(at.actions[0], AlterTableAction::AddColumn(_)));
                assert!(matches!(
                    at.actions[1],
                    AlterTableAction::AddConstraint(TableConstraint::Unique { .. })
                ));
            }
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_alter_table_add_column_with_inline_check() {
        let sql = "ALTER TABLE t ADD COLUMN age int CHECK (age > 0);";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::AlterTable(at) => {
                assert!(at.actions.len() >= 2);
                assert!(matches!(at.actions[0], AlterTableAction::AddColumn(_)));
                assert!(matches!(
                    at.actions[1],
                    AlterTableAction::AddConstraint(TableConstraint::Check { .. })
                ));
            }
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_create_table_with_named_check() {
        let sql = "CREATE TABLE t (col int, CONSTRAINT chk_col CHECK (col > 0));";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                let check = ct
                    .constraints
                    .iter()
                    .find(|c| matches!(c, TableConstraint::Check { .. }));
                assert!(check.is_some());
                match check.unwrap() {
                    TableConstraint::Check { name, .. } => {
                        assert_eq!(name.as_deref(), Some("chk_col"));
                    }
                    _ => unreachable!(),
                }
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_drop_function_ignored() {
        let sql = "DROP FUNCTION my_func(int);";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        assert!(matches!(nodes[0].node, IrNode::Ignored { .. }));
    }

    #[test]
    fn test_parse_create_schema_ignored() {
        let sql = "CREATE SCHEMA myschema;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        assert!(matches!(nodes[0].node, IrNode::Ignored { .. }));
    }

    #[test]
    fn test_parse_alter_sequence_ignored() {
        let sql = "ALTER SEQUENCE order_seq RESTART WITH 1000;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        assert!(matches!(nodes[0].node, IrNode::Ignored { .. }));
    }

    // -----------------------------------------------------------------------
    // relation_to_qualified_name with None input
    // -----------------------------------------------------------------------

    #[test]
    fn test_relation_to_qualified_name_none() {
        let name = relation_to_qualified_name(None);
        assert_eq!(name.name, "unknown");
    }

    // -----------------------------------------------------------------------
    // relation_persistence with None input
    // -----------------------------------------------------------------------

    #[test]
    fn test_relation_persistence_none() {
        assert_eq!(relation_persistence(None), TablePersistence::Permanent);
    }

    // -----------------------------------------------------------------------
    // extract_table_hint_from_raw — additional coverage
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_table_hint_no_match() {
        let hint = extract_table_hint_from_raw("SELECT 1;");
        assert!(hint.is_none());
    }

    #[test]
    fn test_extract_table_hint_empty() {
        let hint = extract_table_hint_from_raw("");
        assert!(hint.is_none());
    }

    // -----------------------------------------------------------------------
    // CLUSTER statement
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_cluster_with_index() {
        let sql = "CLUSTER customers USING idx_customers_email;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::Cluster(c) => {
                assert_eq!(c.table, QualifiedName::unqualified("customers"));
                assert_eq!(c.index.as_deref(), Some("idx_customers_email"));
            }
            other => panic!("Expected Cluster, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_cluster_without_index() {
        let sql = "CLUSTER events;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::Cluster(c) => {
                assert_eq!(c.table, QualifiedName::unqualified("events"));
                assert!(c.index.is_none());
            }
            other => panic!("Expected Cluster, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_cluster_schema_qualified() {
        let sql = "CLUSTER myschema.orders USING idx_orders_id;";
        let nodes = parse_sql(sql);
        assert_eq!(nodes.len(), 1);
        match &nodes[0].node {
            IrNode::Cluster(c) => {
                assert_eq!(c.table.schema.as_deref(), Some("myschema"));
                assert_eq!(c.table.name, "orders");
                assert_eq!(c.index.as_deref(), Some("idx_orders_id"));
            }
            other => panic!("Expected Cluster, got: {:?}", other),
        }
    }
}
