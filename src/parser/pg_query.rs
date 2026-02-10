//! pg_query AST to IR conversion
//!
//! This module converts the pg_query crate's PostgreSQL AST into the simplified
//! IR layer used by the rule engine. It handles type canonicalization, constraint
//! normalization, and source location tracking.

use crate::parser::ir::{
    AlterTable, AlterTableAction, ColumnDef, CreateIndex, CreateTable, DefaultExpr, DropIndex,
    DropTable, IndexColumn, IrNode, Located, QualifiedName, SourceSpan, TableConstraint, TypeName,
};
use pg_query::NodeEnum;

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

        let stmt_node = raw_stmt
            .stmt
            .as_ref()
            .and_then(|s| s.node.as_ref());

        let ir_node = match stmt_node {
            Some(node_enum) => convert_node(node_enum, &raw_sql),
            None => IrNode::Ignored {
                raw_sql: raw_sql.clone(),
            },
        };

        nodes.push(Located {
            node: ir_node,
            span: SourceSpan {
                start_line,
                end_line,
                start_offset,
                end_offset,
            },
        });
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

/// Convert a pg_query `NodeEnum` into an IR node.
fn convert_node(node: &NodeEnum, raw_sql: &str) -> IrNode {
    match node {
        NodeEnum::CreateStmt(create) => convert_create_table(create, raw_sql),
        NodeEnum::AlterTableStmt(alter) => convert_alter_table(alter, raw_sql),
        NodeEnum::IndexStmt(idx) => convert_create_index(idx),
        NodeEnum::DropStmt(drop) => convert_drop_stmt(drop, raw_sql),
        NodeEnum::DoStmt(_) => IrNode::Unparseable {
            raw_sql: raw_sql.to_string(),
            table_hint: None,
        },
        _ => IrNode::Ignored {
            raw_sql: raw_sql.to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// CREATE TABLE
// ---------------------------------------------------------------------------

/// Convert a pg_query `CreateStmt` to `IrNode::CreateTable`.
fn convert_create_table(
    create: &pg_query::protobuf::CreateStmt,
    raw_sql: &str,
) -> IrNode {
    let name = relation_to_qualified_name(create.relation.as_ref());

    let temporary = matches!(
        create.oncommit(),
        pg_query::protobuf::OnCommitAction::OncommitDrop
            | pg_query::protobuf::OnCommitAction::OncommitDeleteRows
    ) || is_temp_relation(create.relation.as_ref());

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
        temporary,
    })
        .or_unparseable(raw_sql)
}

/// Convert a pg_query `ColumnDef` into an IR `ColumnDef` plus any inline
/// constraints that should be promoted to table-level constraints.
///
/// Returns `(ColumnDef, Vec<TableConstraint>)` where the vector contains
/// inline PRIMARY KEY, FOREIGN KEY, UNIQUE, and CHECK constraints.
fn convert_column_def(
    col: &pg_query::protobuf::ColumnDef,
) -> (ColumnDef, Vec<TableConstraint>) {
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
                });
            }
            pg_query::protobuf::ConstrType::ConstrForeign => {
                let ref_table = relation_to_qualified_name(con.pktable.as_ref());
                let ref_columns = extract_string_list(&con.pk_attrs);
                let name = if con.conname.is_empty() {
                    None
                } else {
                    Some(con.conname.clone())
                };
                constraints.push(TableConstraint::ForeignKey {
                    name,
                    columns: vec![col_name.clone()],
                    ref_table,
                    ref_columns,
                });
            }
            pg_query::protobuf::ConstrType::ConstrUnique => {
                let name = if con.conname.is_empty() {
                    None
                } else {
                    Some(con.conname.clone())
                };
                constraints.push(TableConstraint::Unique {
                    name,
                    columns: vec![col_name.clone()],
                });
            }
            pg_query::protobuf::ConstrType::ConstrCheck => {
                let name = if con.conname.is_empty() {
                    None
                } else {
                    Some(con.conname.clone())
                };
                let expression = con
                    .raw_expr
                    .as_ref()
                    .map(|e| deparse_node(e))
                    .unwrap_or_default();
                constraints.push(TableConstraint::Check { name, expression });
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
    };

    (col_def, constraints)
}

// ---------------------------------------------------------------------------
// Type name extraction
// ---------------------------------------------------------------------------

/// Extract a canonical `TypeName` from a pg_query `TypeName` node.
///
/// Returns `(TypeName, is_serial)` where `is_serial` is true if the original
/// type was `serial` or `bigserial` (which pg_query does NOT expand).
///
/// Canonical name extraction: use the LAST element of `TypeName.names[]`.
/// This normalizes all PostgreSQL type aliases automatically.
fn extract_type_name(
    tn: Option<&pg_query::protobuf::TypeName>,
) -> (TypeName, bool) {
    let tn = match tn {
        Some(t) => t,
        None => return (TypeName::simple("unknown"), false),
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
        .unwrap_or_else(|| "unknown".to_string())
        .to_lowercase();

    // Check for serial types (NOT expanded by pg_query)
    let is_serial = canonical == "serial" || canonical == "bigserial";
    let mapped_name = match canonical.as_str() {
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

            let args: Vec<String> = fc
                .args
                .iter()
                .map(deparse_node)
                .collect();

            DefaultExpr::FunctionCall { name, args }
        }
        Some(NodeEnum::TypeCast(_)) => {
            DefaultExpr::Other(deparse_node(node))
        }
        Some(_) => DefaultExpr::Other(deparse_node(node)),
        None => DefaultExpr::Other("NULL".to_string()),
    }
}

// ---------------------------------------------------------------------------
// ALTER TABLE
// ---------------------------------------------------------------------------

/// Convert a pg_query `AlterTableStmt` to `IrNode::AlterTable`.
fn convert_alter_table(
    alter: &pg_query::protobuf::AlterTableStmt,
    raw_sql: &str,
) -> IrNode {
    let name = relation_to_qualified_name(alter.relation.as_ref());

    let mut actions = Vec::new();

    for cmd_node in &alter.cmds {
        let cmd = match cmd_node.node.as_ref() {
            Some(NodeEnum::AlterTableCmd(c)) => c,
            _ => continue,
        };

        let action = convert_alter_table_cmd(cmd);
        actions.push(action);
    }

    if actions.is_empty() {
        return IrNode::Ignored {
            raw_sql: raw_sql.to_string(),
        };
    }

    IrNode::AlterTable(AlterTable { name, actions })
}

/// Convert a single `AlterTableCmd` into an `AlterTableAction`.
fn convert_alter_table_cmd(
    cmd: &pg_query::protobuf::AlterTableCmd,
) -> AlterTableAction {
    match cmd.subtype() {
        pg_query::protobuf::AlterTableType::AtAddColumn => {
            match cmd.def.as_ref().and_then(|d| d.node.as_ref()) {
                Some(NodeEnum::ColumnDef(col)) => {
                    let (col_def, _inline_constraints) = convert_column_def(col);
                    // Note: inline constraints from ADD COLUMN are not yet
                    // promoted to table-level in the AlterTable IR. The catalog
                    // replay handles AddColumn constraints separately.
                    AlterTableAction::AddColumn(col_def)
                }
                _ => AlterTableAction::Other {
                    description: "ADD COLUMN (unparseable definition)".to_string(),
                },
            }
        }
        pg_query::protobuf::AlterTableType::AtDropColumn => {
            AlterTableAction::DropColumn {
                name: cmd.name.clone(),
            }
        }
        pg_query::protobuf::AlterTableType::AtAddConstraint => {
            match cmd.def.as_ref().and_then(|d| d.node.as_ref()) {
                Some(NodeEnum::Constraint(con)) => {
                    match convert_table_constraint(con, None) {
                        Some(tc) => AlterTableAction::AddConstraint(tc),
                        None => AlterTableAction::Other {
                            description: "ADD CONSTRAINT (unknown type)".to_string(),
                        },
                    }
                }
                _ => AlterTableAction::Other {
                    description: "ADD CONSTRAINT (unparseable)".to_string(),
                },
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
                .unwrap_or_else(|| TypeName::simple("unknown"));

            AlterTableAction::AlterColumnType {
                column_name: cmd.name.clone(),
                new_type,
                old_type: None, // Must be filled in from catalog during linting
            }
        }
        pg_query::protobuf::AlterTableType::AtSetNotNull => {
            AlterTableAction::Other {
                description: format!("SET NOT NULL on {}", cmd.name),
            }
        }
        pg_query::protobuf::AlterTableType::AtDropNotNull => {
            AlterTableAction::Other {
                description: format!("DROP NOT NULL on {}", cmd.name),
            }
        }
        other => AlterTableAction::Other {
            description: format!("{:?}", other),
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
    let name = if con.conname.is_empty() {
        None
    } else {
        Some(con.conname.clone())
    };

    match con.contype() {
        pg_query::protobuf::ConstrType::ConstrPrimary => {
            let mut columns = extract_string_list(&con.keys);
            if columns.is_empty() {
                if let Some(col) = context_column {
                    columns.push(col.to_string());
                }
            }
            Some(TableConstraint::PrimaryKey { columns })
        }
        pg_query::protobuf::ConstrType::ConstrForeign => {
            let ref_table = relation_to_qualified_name(con.pktable.as_ref());
            let ref_columns = extract_string_list(&con.pk_attrs);
            let mut columns = extract_string_list(&con.fk_attrs);
            if columns.is_empty() {
                if let Some(col) = context_column {
                    columns.push(col.to_string());
                }
            }
            Some(TableConstraint::ForeignKey {
                name,
                columns,
                ref_table,
                ref_columns,
            })
        }
        pg_query::protobuf::ConstrType::ConstrUnique => {
            let mut columns = extract_string_list(&con.keys);
            if columns.is_empty() {
                if let Some(col) = context_column {
                    columns.push(col.to_string());
                }
            }
            Some(TableConstraint::Unique { name, columns })
        }
        pg_query::protobuf::ConstrType::ConstrCheck => {
            let expression = con
                .raw_expr
                .as_ref()
                .map(|e| deparse_node(e))
                .unwrap_or_default();
            Some(TableConstraint::Check { name, expression })
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
    })
}

// ---------------------------------------------------------------------------
// DROP statements
// ---------------------------------------------------------------------------

/// Convert a pg_query `DropStmt` to the appropriate IR node.
///
/// - `ObjectType::ObjectIndex` -> `IrNode::DropIndex`
/// - `ObjectType::ObjectTable` -> `IrNode::DropTable`
/// - Everything else -> `IrNode::Ignored`
fn convert_drop_stmt(
    drop: &pg_query::protobuf::DropStmt,
    raw_sql: &str,
) -> IrNode {
    match drop.remove_type() {
        pg_query::protobuf::ObjectType::ObjectIndex => {
            let index_name = extract_name_from_drop_objects(&drop.objects);
            match index_name {
                Some(name) => IrNode::DropIndex(DropIndex {
                    index_name: name,
                    concurrent: drop.concurrent,
                }),
                None => IrNode::Ignored {
                    raw_sql: raw_sql.to_string(),
                },
            }
        }
        pg_query::protobuf::ObjectType::ObjectTable => {
            let table_name = extract_qualified_name_from_drop_objects(&drop.objects);
            match table_name {
                Some(name) => IrNode::DropTable(DropTable { name }),
                None => IrNode::Ignored {
                    raw_sql: raw_sql.to_string(),
                },
            }
        }
        _ => IrNode::Ignored {
            raw_sql: raw_sql.to_string(),
        },
    }
}

/// Extract the object name from `DropStmt.objects[]`.
///
/// For `DROP INDEX`, the objects list contains `List { items: [String(name)] }`.
/// We take the last string element to get the unqualified name.
fn extract_name_from_drop_objects(objects: &[pg_query::protobuf::Node]) -> Option<String> {
    for obj in objects {
        if let Some(NodeEnum::List(list)) = obj.node.as_ref() {
            // Take the last string item as the name
            for item in list.items.iter().rev() {
                if let Some(NodeEnum::String(s)) = item.node.as_ref() {
                    return Some(s.sval.clone());
                }
            }
        }
    }
    None
}

/// Extract a qualified name from `DropStmt.objects[]` for DROP TABLE.
///
/// Handles both `DROP TABLE foo` (unqualified) and `DROP TABLE myschema.foo` (qualified).
fn extract_qualified_name_from_drop_objects(
    objects: &[pg_query::protobuf::Node],
) -> Option<QualifiedName> {
    for obj in objects {
        if let Some(NodeEnum::List(list)) = obj.node.as_ref() {
            let strings: Vec<String> = list
                .items
                .iter()
                .filter_map(|item| match item.node.as_ref() {
                    Some(NodeEnum::String(s)) => Some(s.sval.clone()),
                    _ => None,
                })
                .collect();

            return match strings.len() {
                1 => Some(QualifiedName::unqualified(&strings[0])),
                2 => Some(QualifiedName::qualified(&strings[0], &strings[1])),
                _ if !strings.is_empty() => {
                    // Take last two as schema.name
                    let name = strings.last().cloned().unwrap_or_default();
                    if strings.len() >= 2 {
                        let schema = strings[strings.len() - 2].clone();
                        Some(QualifiedName::qualified(schema, name))
                    } else {
                        Some(QualifiedName::unqualified(name))
                    }
                }
                _ => None,
            };
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a pg_query `RangeVar` (relation reference) to a `QualifiedName`.
fn relation_to_qualified_name(
    rel: Option<&pg_query::protobuf::RangeVar>,
) -> QualifiedName {
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

/// Check if a `RangeVar` refers to a temporary table.
///
/// In pg_query, temporary tables are indicated by the `relpersistence` field
/// being set to `'t'` (temporary) or `'u'` (unlogged).
fn is_temp_relation(rel: Option<&pg_query::protobuf::RangeVar>) -> bool {
    match rel {
        Some(r) => r.relpersistence == "t",
        None => false,
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
    if let Some(stmt) = parse_result.protobuf.stmts.first_mut() {
        if let Some(ref mut stmt_node) = stmt.stmt {
            if let Some(NodeEnum::SelectStmt(ref mut select)) = stmt_node.node {
                if let Some(first_target) = select.target_list.first_mut() {
                    if let Some(NodeEnum::ResTarget(ref mut res)) = first_target.node {
                        res.val = Some(Box::new(node.clone()));
                    }
                }
            }
        }
    }

    match pg_query::deparse(&parse_result.protobuf) {
        Ok(sql) => {
            // Strip the "SELECT " prefix
            sql.strip_prefix("SELECT ")
                .unwrap_or(&sql)
                .to_string()
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
    if let Some(pos) = upper.find("ALTER TABLE") {
        let rest = &sql[pos + 11..];
        return extract_first_identifier(rest);
    }

    // Try to find "CREATE TABLE <name>" pattern
    if let Some(pos) = upper.find("CREATE TABLE") {
        let rest = &sql[pos + 12..];
        return extract_first_identifier(rest);
    }

    None
}

/// Extract the first SQL identifier from a string, skipping whitespace and keywords.
fn extract_first_identifier(s: &str) -> Option<String> {
    let trimmed = s.trim();

    // Skip "IF NOT EXISTS" / "IF EXISTS"
    let trimmed = if trimmed.to_uppercase().starts_with("IF NOT EXISTS") {
        trimmed[13..].trim()
    } else if trimmed.to_uppercase().starts_with("IF EXISTS") {
        trimmed[9..].trim()
    } else if trimmed.to_uppercase().starts_with("ONLY") {
        trimmed[4..].trim()
    } else {
        trimmed
    };

    // Take characters that could be part of an identifier (letters, digits, _, .)
    let ident: String = trimmed
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.' || *c == '"')
        .collect();

    if ident.is_empty() {
        None
    } else {
        // Strip quotes and return the last component (table name)
        let cleaned = ident.replace('"', "");
        let parts: Vec<&str> = cleaned.split('.').collect();
        parts.last().map(|s| s.to_string())
    }
}

/// Extension trait to allow fallback to `Unparseable` if conversion panics.
/// This is used internally as a safe guard.
trait OrUnparseable {
    fn or_unparseable(self, raw_sql: &str) -> IrNode;
}

impl OrUnparseable for IrNode {
    fn or_unparseable(self, _raw_sql: &str) -> IrNode {
        self
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
                assert_eq!(ct.columns[0].default_expr, Some(DefaultExpr::Literal("0".to_string())));
            }
            other => panic!("Expected CreateTable, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_default_function_call() {
        let sql = "CREATE TABLE t (col timestamp DEFAULT now());";
        let nodes = parse_sql(sql);
        match &nodes[0].node {
            IrNode::CreateTable(ct) => {
                match &ct.columns[0].default_expr {
                    Some(DefaultExpr::FunctionCall { name, .. }) => {
                        assert_eq!(name, "now");
                    }
                    other => panic!("Expected FunctionCall default, got: {:?}", other),
                }
            }
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
                        TableConstraint::PrimaryKey { columns } if columns == &["id"]
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
                assert!(
                    ct.constraints.iter().any(|c| matches!(
                        c,
                        TableConstraint::PrimaryKey { columns } if columns == &["id"]
                    )),
                );
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
                assert!(
                    ct.constraints.iter().any(|c| matches!(
                        c,
                        TableConstraint::Unique { columns, .. } if columns == &["email"]
                    )),
                );
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
                    AlterTableAction::AddConstraint(tc) => {
                        match tc {
                            TableConstraint::ForeignKey {
                                name, columns, ref_table, ref_columns,
                            } => {
                                assert_eq!(name.as_deref(), Some("fk_customer"));
                                assert_eq!(columns, &["customer_id"]);
                                assert_eq!(ref_table, &QualifiedName::unqualified("customers"));
                                assert_eq!(ref_columns, &["id"]);
                            }
                            other => panic!("Expected ForeignKey, got: {:?}", other),
                        }
                    }
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
                    AlterTableAction::Other { description } => {
                        assert!(
                            description.contains("SET NOT NULL"),
                            "Expected 'SET NOT NULL' in description, got: {}",
                            description
                        );
                    }
                    other => panic!("Expected Other, got: {:?}", other),
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
                assert!(ct.columns[0].nullable, "Column without NOT NULL should be nullable");
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
                assert!(!ct.columns[0].nullable, "Column with NOT NULL should not be nullable");
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
                assert!(!ct.columns[0].nullable, "PRIMARY KEY column should not be nullable");
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
                    ct.constraints.iter().any(|c| matches!(c, TableConstraint::Check { .. })),
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
            IrNode::AlterTable(at) => {
                match &at.actions[0] {
                    AlterTableAction::AddColumn(col) => {
                        assert_eq!(col.type_name.name, "int4");
                        assert!(matches!(
                            col.default_expr,
                            Some(DefaultExpr::FunctionCall { ref name, .. }) if name == "nextval"
                        ));
                    }
                    other => panic!("Expected AddColumn, got: {:?}", other),
                }
            }
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
            IrNode::CreateTable(ct) => {
                match &ct.columns[0].default_expr {
                    Some(DefaultExpr::Literal(v)) => {
                        assert_eq!(v, "true");
                    }
                    other => panic!("Expected Literal default, got: {:?}", other),
                }
            }
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
            IrNode::AlterTable(at) => {
                match &at.actions[0] {
                    AlterTableAction::AddConstraint(TableConstraint::PrimaryKey { columns }) => {
                        assert_eq!(columns, &["id"]);
                    }
                    other => panic!("Expected AddConstraint PrimaryKey, got: {:?}", other),
                }
            }
            other => panic!("Expected AlterTable, got: {:?}", other),
        }
    }
}
