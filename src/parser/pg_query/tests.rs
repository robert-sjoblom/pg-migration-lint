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
            assert_eq!(ci.columns[0], IndexColumn::Column("status".to_string()));
            assert!(!ci.unique);
            assert!(!ci.concurrent);
            assert!(!ci.if_not_exists);
            assert!(ci.where_clause.is_none());
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
            assert_eq!(
                ci.columns[0],
                IndexColumn::Column("customer_id".to_string())
            );
            assert_eq!(ci.columns[1], IndexColumn::Column("status".to_string()));
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
// DROP SCHEMA
// -----------------------------------------------------------------------

#[test]
fn test_parse_drop_schema_cascade() {
    let sql = "DROP SCHEMA myschema CASCADE;";
    let nodes = parse_sql(sql);
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::DropSchema(ds) => {
            assert_eq!(ds.schema_name, "myschema");
            assert!(ds.cascade);
            assert!(!ds.if_exists);
        }
        other => panic!("Expected DropSchema, got: {:?}", other),
    }
}

#[test]
fn test_parse_drop_schema_if_exists_no_cascade() {
    let sql = "DROP SCHEMA IF EXISTS myschema;";
    let nodes = parse_sql(sql);
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::DropSchema(ds) => {
            assert_eq!(ds.schema_name, "myschema");
            assert!(!ds.cascade);
            assert!(ds.if_exists);
        }
        other => panic!("Expected DropSchema, got: {:?}", other),
    }
}

#[test]
fn test_parse_drop_schema_if_exists_cascade() {
    let sql = "DROP SCHEMA IF EXISTS myschema CASCADE;";
    let nodes = parse_sql(sql);
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::DropSchema(ds) => {
            assert_eq!(ds.schema_name, "myschema");
            assert!(ds.cascade);
            assert!(ds.if_exists);
        }
        other => panic!("Expected DropSchema, got: {:?}", other),
    }
}

#[test]
fn test_parse_drop_schema_plain() {
    let sql = "DROP SCHEMA myschema;";
    let nodes = parse_sql(sql);
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::DropSchema(ds) => {
            assert_eq!(ds.schema_name, "myschema");
            assert!(!ds.cascade);
            assert!(!ds.if_exists);
        }
        other => panic!("Expected DropSchema, got: {:?}", other),
    }
}

#[test]
fn test_parse_drop_schema_multiple_schemas() {
    let sql = "DROP SCHEMA foo, bar CASCADE;";
    let nodes = parse_sql(sql);
    assert_eq!(nodes.len(), 2);
    match &nodes[0].node {
        IrNode::DropSchema(ds) => {
            assert_eq!(ds.schema_name, "foo");
            assert!(ds.cascade);
        }
        other => panic!("Expected DropSchema, got: {:?}", other),
    }
    match &nodes[1].node {
        IrNode::DropSchema(ds) => {
            assert_eq!(ds.schema_name, "bar");
            assert!(ds.cascade);
        }
        other => panic!("Expected DropSchema, got: {:?}", other),
    }
}

#[test]
fn test_parse_drop_schema_restrict() {
    let sql = "DROP SCHEMA myschema RESTRICT;";
    let nodes = parse_sql(sql);
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::DropSchema(ds) => {
            assert_eq!(ds.schema_name, "myschema");
            assert!(!ds.cascade, "RESTRICT should map to cascade=false");
            assert!(!ds.if_exists);
        }
        other => panic!("Expected DropSchema, got: {:?}", other),
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
                    not_valid, ..
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
                    name, not_valid, ..
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
                AlterTableAction::AddConstraint(TableConstraint::Check { not_valid, .. }) => {
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
                AlterTableAction::DropNotNull { column_name } => {
                    assert_eq!(column_name, "bar");
                }
                other => panic!("Expected DropNotNull action, got: {:?}", other),
            }
        }
        other => panic!("Expected AlterTable, got: {:?}", other),
    }
}

#[test]
fn test_parse_alter_table_drop_constraint() {
    let sql = "ALTER TABLE orders DROP CONSTRAINT fk_customer;";
    let nodes = parse_sql(sql);
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::AlterTable(at) => {
            assert_eq!(at.name.name, "orders");
            assert_eq!(at.actions.len(), 1);
            match &at.actions[0] {
                AlterTableAction::DropConstraint { constraint_name } => {
                    assert_eq!(constraint_name, "fk_customer");
                }
                other => panic!("Expected DropConstraint action, got: {:?}", other),
            }
        }
        other => panic!("Expected AlterTable, got: {:?}", other),
    }
}

#[test]
fn test_parse_alter_table_validate_constraint() {
    let sql = "ALTER TABLE orders VALIDATE CONSTRAINT fk_customer;";
    let nodes = parse_sql(sql);
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::AlterTable(at) => {
            assert_eq!(at.name.name, "orders");
            assert_eq!(at.actions.len(), 1);
            match &at.actions[0] {
                AlterTableAction::ValidateConstraint { constraint_name } => {
                    assert_eq!(constraint_name, "fk_customer");
                }
                other => panic!("Expected ValidateConstraint action, got: {:?}", other),
            }
        }
        other => panic!("Expected AlterTable, got: {:?}", other),
    }
}

#[test]
fn test_parse_alter_table_drop_constraint_if_exists() {
    let sql = "ALTER TABLE orders DROP CONSTRAINT IF EXISTS fk_customer;";
    let nodes = parse_sql(sql);
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::AlterTable(at) => {
            assert_eq!(at.name.name, "orders");
            assert_eq!(at.actions.len(), 1);
            match &at.actions[0] {
                AlterTableAction::DropConstraint { constraint_name } => {
                    assert_eq!(constraint_name, "fk_customer");
                }
                other => panic!("Expected DropConstraint action, got: {:?}", other),
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

// -----------------------------------------------------------------------
// Tests: Partial and expression indexes
// -----------------------------------------------------------------------

#[test]
fn test_parse_expression_index() {
    let sql = "CREATE INDEX idx_email_lower ON users (LOWER(email));";
    let nodes = parse_sql(sql);
    match &nodes[0].node {
        IrNode::CreateIndex(ci) => {
            assert_eq!(ci.columns.len(), 1);
            assert!(
                matches!(
                    &ci.columns[0],
                    IndexColumn::Expression { text, referenced_columns }
                    if text == "lower(email)" && referenced_columns == &["email".to_string()]
                ),
                "Expected Expression {{ text: lower(email), referenced_columns: [email] }}, got: {:?}",
                ci.columns[0]
            );
            assert!(ci.where_clause.is_none());
        }
        other => panic!("Expected CreateIndex, got: {:?}", other),
    }
}

#[test]
fn test_parse_mixed_column_expression_index() {
    let sql = "CREATE INDEX idx_tenant_email ON users (tenant_id, LOWER(email));";
    let nodes = parse_sql(sql);
    match &nodes[0].node {
        IrNode::CreateIndex(ci) => {
            assert_eq!(ci.columns.len(), 2);
            assert_eq!(ci.columns[0], IndexColumn::Column("tenant_id".to_string()));
            assert!(
                matches!(
                    &ci.columns[1],
                    IndexColumn::Expression { text, referenced_columns }
                    if text == "lower(email)" && referenced_columns == &["email".to_string()]
                ),
                "Expected Expression {{ text: lower(email), referenced_columns: [email] }}, got: {:?}",
                ci.columns[1]
            );
        }
        other => panic!("Expected CreateIndex, got: {:?}", other),
    }
}

#[test]
fn test_parse_partial_index_where_clause() {
    let sql = "CREATE INDEX idx_status ON orders (status) WHERE active = true;";
    let nodes = parse_sql(sql);
    match &nodes[0].node {
        IrNode::CreateIndex(ci) => {
            assert_eq!(ci.columns.len(), 1);
            assert_eq!(ci.columns[0], IndexColumn::Column("status".to_string()));
            assert!(ci.where_clause.is_some(), "Should have a WHERE clause");
            let wc = ci.where_clause.as_deref().unwrap();
            assert!(
                wc.contains("active") && wc.contains("true"),
                "WHERE clause should contain 'active' and 'true', got: {}",
                wc
            );
        }
        other => panic!("Expected CreateIndex, got: {:?}", other),
    }
}

#[test]
fn test_parse_partial_unique_index() {
    let sql = "CREATE UNIQUE INDEX idx_email ON users (email) WHERE deleted_at IS NULL;";
    let nodes = parse_sql(sql);
    match &nodes[0].node {
        IrNode::CreateIndex(ci) => {
            assert!(ci.unique);
            assert_eq!(ci.columns.len(), 1);
            assert_eq!(ci.columns[0], IndexColumn::Column("email".to_string()));
            assert!(ci.where_clause.is_some(), "Should have a WHERE clause");
            let wc = ci.where_clause.as_deref().unwrap();
            assert!(
                wc.contains("deleted_at") && wc.contains("IS NULL"),
                "WHERE clause should contain 'deleted_at' and 'IS NULL', got: {}",
                wc
            );
        }
        other => panic!("Expected CreateIndex, got: {:?}", other),
    }
}

// -----------------------------------------------------------------------
// Tests: extract_column_refs via expression index parsing
// -----------------------------------------------------------------------

/// Helper: parse a CREATE INDEX with the given expression and return the
/// referenced_columns from the first (expression) column.
fn expr_index_refs(expr: &str) -> Vec<String> {
    let sql = format!("CREATE INDEX idx ON t ({});", expr);
    let nodes = parse_sql(&sql);
    match &nodes[0].node {
        IrNode::CreateIndex(ci) => match &ci.columns[0] {
            IndexColumn::Expression {
                referenced_columns, ..
            } => referenced_columns.clone(),
            IndexColumn::Column(name) => {
                panic!("Expected Expression, got Column({name})")
            }
        },
        other => panic!("Expected CreateIndex, got: {:?}", other),
    }
}

#[test]
fn test_extract_refs_lower_email() {
    assert_eq!(expr_index_refs("LOWER(email)"), vec!["email"]);
}

#[test]
fn test_extract_refs_typecast() {
    // PostgreSQL requires parentheses around non-function expressions in indexes.
    assert_eq!(expr_index_refs("(email::text)"), vec!["email"]);
}

#[test]
fn test_extract_refs_coalesce() {
    let refs = expr_index_refs("COALESCE(a, b)");
    assert_eq!(refs, vec!["a", "b"]);
}

#[test]
fn test_extract_refs_arithmetic() {
    // PostgreSQL requires parentheses around operator expressions in indexes.
    assert_eq!(expr_index_refs("(a + 1)"), vec!["a"]);
}

#[test]
fn test_extract_refs_constants_only() {
    // PostgreSQL requires parentheses around operator expressions in indexes.
    assert_eq!(expr_index_refs("(1 + 2)"), Vec::<String>::new());
}

#[test]
fn test_extract_refs_nested_func_typecast() {
    assert_eq!(expr_index_refs("LOWER(email::text)"), vec!["email"]);
}

#[test]
fn test_extract_refs_multi_arg_func() {
    let refs = expr_index_refs("COALESCE(first_name, last_name, 'unknown')");
    assert_eq!(refs, vec!["first_name", "last_name"]);
}

#[test]
fn test_extract_refs_bool_expr() {
    let refs = expr_index_refs("(a > 0 AND b > 0)");
    assert_eq!(refs, vec!["a", "b"]);
}

#[test]
fn test_extract_refs_case_with_default() {
    let refs = expr_index_refs("(CASE WHEN status = 'active' THEN priority ELSE 0 END)");
    assert_eq!(refs, vec!["priority", "status"]);
}

#[test]
fn test_extract_refs_case_simple_form() {
    let refs = expr_index_refs("(CASE status WHEN 'a' THEN priority ELSE rank END)");
    assert_eq!(refs, vec!["priority", "rank", "status"]);
}

#[test]
fn test_extract_refs_null_test_in_case() {
    let refs = expr_index_refs("(CASE WHEN email IS NOT NULL THEN email ELSE 'none' END)");
    assert_eq!(refs, vec!["email"]);
}

#[test]
fn test_extract_refs_greatest() {
    let refs = expr_index_refs("GREATEST(a, b)");
    assert_eq!(refs, vec!["a", "b"]);
}

#[test]
fn test_extract_refs_least() {
    let refs = expr_index_refs("LEAST(x, y, z)");
    assert_eq!(refs, vec!["x", "y", "z"]);
}

#[test]
fn test_extract_refs_dedup() {
    let refs = expr_index_refs("(a + a)");
    assert_eq!(refs, vec!["a"]);
}

// -----------------------------------------------------------------------
// Partition support tests
// -----------------------------------------------------------------------

#[test]
fn test_partition_by_range() {
    let nodes =
        parse_sql("CREATE TABLE measurements (id int, ts timestamptz) PARTITION BY RANGE (ts);");
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::CreateTable(ct) => {
            let pb = ct
                .partition_by
                .as_ref()
                .expect("partition_by should be set");
            assert_eq!(pb.strategy, PartitionStrategy::Range);
            assert_eq!(pb.columns, vec!["ts".to_string()]);
            assert!(ct.partition_of.is_none());
        }
        other => panic!("Expected CreateTable, got {:?}", other),
    }
}

#[test]
fn test_partition_by_list() {
    let nodes =
        parse_sql("CREATE TABLE sales (region text, amount int) PARTITION BY LIST (region);");
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::CreateTable(ct) => {
            let pb = ct
                .partition_by
                .as_ref()
                .expect("partition_by should be set");
            assert_eq!(pb.strategy, PartitionStrategy::List);
            assert_eq!(pb.columns, vec!["region".to_string()]);
        }
        other => panic!("Expected CreateTable, got {:?}", other),
    }
}

#[test]
fn test_partition_by_hash() {
    let nodes = parse_sql("CREATE TABLE data (id int) PARTITION BY HASH (id);");
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::CreateTable(ct) => {
            let pb = ct
                .partition_by
                .as_ref()
                .expect("partition_by should be set");
            assert_eq!(pb.strategy, PartitionStrategy::Hash);
            assert_eq!(pb.columns, vec!["id".to_string()]);
        }
        other => panic!("Expected CreateTable, got {:?}", other),
    }
}

#[test]
fn test_partition_of() {
    let nodes = parse_sql(
        "CREATE TABLE measurements_2024 PARTITION OF measurements FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');",
    );
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::CreateTable(ct) => {
            assert!(ct.partition_by.is_none());
            let parent = ct
                .partition_of
                .as_ref()
                .expect("partition_of should be set");
            assert_eq!(parent.name, "measurements");
            assert!(parent.schema.is_none());
        }
        other => panic!("Expected CreateTable, got {:?}", other),
    }
}

#[test]
fn test_attach_partition() {
    let nodes = parse_sql(
        "ALTER TABLE measurements ATTACH PARTITION measurements_2024 FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');",
    );
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::AlterTable(at) => {
            assert_eq!(at.name.name, "measurements");
            assert_eq!(at.actions.len(), 1);
            match &at.actions[0] {
                AlterTableAction::AttachPartition { child } => {
                    assert_eq!(child.name, "measurements_2024");
                }
                other => panic!("Expected AttachPartition, got {:?}", other),
            }
        }
        other => panic!("Expected AlterTable, got {:?}", other),
    }
}

#[test]
fn test_detach_partition() {
    let nodes = parse_sql("ALTER TABLE measurements DETACH PARTITION measurements_2024;");
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::AlterTable(at) => {
            assert_eq!(at.actions.len(), 1);
            match &at.actions[0] {
                AlterTableAction::DetachPartition { child, concurrent } => {
                    assert_eq!(child.name, "measurements_2024");
                    assert!(!concurrent);
                }
                other => panic!("Expected DetachPartition, got {:?}", other),
            }
        }
        other => panic!("Expected AlterTable, got {:?}", other),
    }
}

#[test]
fn test_detach_partition_concurrently() {
    let nodes =
        parse_sql("ALTER TABLE measurements DETACH PARTITION measurements_2024 CONCURRENTLY;");
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::AlterTable(at) => {
            assert_eq!(at.actions.len(), 1);
            match &at.actions[0] {
                AlterTableAction::DetachPartition { child, concurrent } => {
                    assert_eq!(child.name, "measurements_2024");
                    assert!(concurrent);
                }
                other => panic!("Expected DetachPartition, got {:?}", other),
            }
        }
        other => panic!("Expected AlterTable, got {:?}", other),
    }
}

#[test]
fn test_create_index_only() {
    let nodes = parse_sql("CREATE INDEX idx_ts ON ONLY measurements (ts);");
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::CreateIndex(ci) => {
            assert!(ci.only, "ONLY should be true");
            assert_eq!(ci.table_name.name, "measurements");
        }
        other => panic!("Expected CreateIndex, got {:?}", other),
    }
}

#[test]
fn test_create_index_normal_not_only() {
    let nodes = parse_sql("CREATE INDEX idx_foo ON foo (bar);");
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::CreateIndex(ci) => {
            assert!(!ci.only, "ONLY should be false for normal index");
        }
        other => panic!("Expected CreateIndex, got {:?}", other),
    }
}

#[test]
fn test_partition_by_expression() {
    let nodes = parse_sql(
        "CREATE TABLE events (id int, ts timestamptz) PARTITION BY RANGE (EXTRACT(YEAR FROM ts));",
    );
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::CreateTable(ct) => {
            let pb = ct
                .partition_by
                .as_ref()
                .expect("partition_by should be set");
            assert_eq!(pb.strategy, PartitionStrategy::Range);
            assert_eq!(pb.columns.len(), 1);
            // Expression key is deparsed into SQL text
            assert!(
                pb.columns[0].contains("EXTRACT")
                    || pb.columns[0].contains("extract")
                    || pb.columns[0].contains("date_part"),
                "Expression partition key should contain function text, got: {}",
                pb.columns[0]
            );
        }
        other => panic!("Expected CreateTable, got {:?}", other),
    }
}

// -----------------------------------------------------------------------
// ALTER INDEX
// -----------------------------------------------------------------------

#[test]
fn test_alter_index_attach_partition() {
    let sql = "ALTER INDEX idx_parent ATTACH PARTITION idx_child;";
    let nodes = parse_sql(sql);
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::AlterIndexAttachPartition {
            parent_index_name,
            child_index_name,
        } => {
            assert_eq!(parent_index_name, "idx_parent");
            assert_eq!(child_index_name.name, "idx_child");
            assert!(child_index_name.schema.is_none());
        }
        other => panic!("Expected AlterIndexAttachPartition, got {:?}", other),
    }
}

#[test]
fn test_alter_index_attach_partition_schema_qualified() {
    let sql = "ALTER INDEX myschema.idx_parent ATTACH PARTITION myschema.idx_child;";
    let nodes = parse_sql(sql);
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::AlterIndexAttachPartition {
            parent_index_name,
            child_index_name,
        } => {
            // pg_query puts the parent name in relation.relname (without schema for index name lookup)
            assert_eq!(parent_index_name, "idx_parent");
            assert_eq!(child_index_name.name, "idx_child");
            assert_eq!(child_index_name.schema.as_deref(), Some("myschema"));
        }
        other => panic!("Expected AlterIndexAttachPartition, got {:?}", other),
    }
}

#[test]
fn test_alter_index_set_ignored() {
    let sql = "ALTER INDEX idx_foo SET (fillfactor = 70);";
    let nodes = parse_sql(sql);
    assert_eq!(nodes.len(), 1);
    assert!(
        matches!(&nodes[0].node, IrNode::Ignored { .. }),
        "ALTER INDEX SET should map to Ignored, got: {:?}",
        nodes[0].node
    );
}
