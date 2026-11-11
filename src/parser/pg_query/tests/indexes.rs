use rstest::rstest;

use crate::{
    IrNode,
    parser::{
        IndexColumn, QualifiedName,
        ir::{CreateIndex, ReindexObjectKind, ReindexTarget},
        pg_query::parse_sql,
    },
};

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

fn parsed_create_index(sql: &str) -> CreateIndex<QualifiedName> {
    match parse_sql(sql)
        .into_iter()
        .next()
        .map(|located| located.node)
    {
        Some(IrNode::CreateIndex(ci)) => ci,
        other => panic!("Expected CreateIndex for `{sql}`, got: {other:?}"),
    }
}

fn generated_name_parts(sql: &str) -> Vec<String> {
    parsed_create_index(sql)
        .columns
        .iter()
        .map(|column| column.generated_name_part().to_owned())
        .collect()
}

fn parsed_drop_index_names(sql: &str) -> Vec<QualifiedName> {
    parse_sql(sql)
        .into_iter()
        .map(|located| match located.node {
            IrNode::DropIndex(di) => di.index_name,
            other => panic!("Expected DropIndex for `{sql}`, got: {other:?}"),
        })
        .collect()
}

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
            assert!(!ci.only, "ONLY should be false for normal index");
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

#[test]
fn test_parse_drop_index_multi() {
    let sql = "DROP INDEX idx1, idx2;";
    let nodes = parse_sql(sql);
    assert_eq!(nodes.len(), 2);
    match &nodes[0].node {
        IrNode::DropIndex(di) => {
            assert_eq!(di.index_name.name, "idx1");
        }
        other => panic!("Expected DropIndex, got: {:?}", other),
    }
    match &nodes[1].node {
        IrNode::DropIndex(di) => {
            assert_eq!(di.index_name.name, "idx2");
        }
        other => panic!("Expected DropIndex, got: {:?}", other),
    }
}

#[rstest]
#[case::plain("DROP INDEX foo;", "foo", false, false)]
#[case::if_exists("DROP INDEX IF EXISTS foo;", "foo", false, true)]
#[case::concurrently_if_exists("DROP INDEX CONCURRENTLY IF EXISTS foo;", "foo", true, true)]
#[case::plain_idx_status("DROP INDEX idx_status;", "idx_status", false, false)]
#[case::concurrently("DROP INDEX CONCURRENTLY idx_status;", "idx_status", true, false)]
#[case::schema_qualified("DROP INDEX myschema.idx_status;", "idx_status", false, false)]
#[case::schema_qualified_with_flags(
    "DROP INDEX CONCURRENTLY IF EXISTS myschema.idx_foo;",
    "idx_foo",
    true,
    true
)]
#[case::if_exists_idx_foo("DROP INDEX IF EXISTS idx_foo;", "idx_foo", false, true)]
fn test_parse_drop_index_flags(
    #[case] sql: &str,
    #[case] expected_index_name: &str,
    #[case] concurrent: bool,
    #[case] if_exists: bool,
) {
    let nodes = parse_sql(sql);
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::DropIndex(di) => {
            assert_eq!(
                di.index_name.name, expected_index_name,
                "index_name mismatch for: {sql}"
            );
            assert_eq!(di.concurrent, concurrent, "concurrent mismatch for: {sql}");
            assert_eq!(di.if_exists, if_exists, "if_exists mismatch for: {sql}");
        }
        other => panic!("Expected DropIndex for `{sql}`, got: {other:?}"),
    }
}

#[test]
fn test_parse_drop_index_multi_schema_qualified() {
    assert_eq!(
        parsed_drop_index_names("DROP INDEX myschema.idx1, myschema.idx2;"),
        vec![
            QualifiedName::qualified("myschema", "idx1"),
            QualifiedName::qualified("myschema", "idx2"),
        ]
    );
}

#[test]
fn test_parse_drop_index_schema_qualified_keeps_schema() {
    assert_eq!(
        parsed_drop_index_names("DROP INDEX myschema.idx_status;"),
        vec![QualifiedName::qualified("myschema", "idx_status")]
    );
}

#[test]
fn test_parse_drop_index_unqualified_has_no_schema() {
    assert_eq!(
        parsed_drop_index_names("DROP INDEX idx_status;"),
        vec![QualifiedName::unqualified("idx_status")]
    );
}

#[test]
fn test_parse_drop_index_multi_mixed_qualification() {
    assert_eq!(
        parsed_drop_index_names("DROP INDEX a.x, y;"),
        vec![
            QualifiedName::qualified("a", "x"),
            QualifiedName::unqualified("y"),
        ]
    );
}

// TODO: can we find out the name they're given?
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
fn test_parse_expression_index() {
    let sql = "CREATE INDEX idx_email_lower ON users (LOWER(email));";
    let nodes = parse_sql(sql);
    match &nodes[0].node {
        IrNode::CreateIndex(ci) => {
            assert_eq!(ci.columns.len(), 1);
            assert!(
                matches!(
                    &ci.columns[0],
                    IndexColumn::Expression { text, referenced_columns, .. }
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
                    IndexColumn::Expression { text, referenced_columns, .. }
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

#[rstest]
#[case::lower_email("LOWER(email)", &["email"])]
#[case::typecast("(email::text)", &["email"])]
#[case::coalesce("COALESCE(a, b)", &["a", "b"])]
#[case::arithmetic("(a + 1)", &["a"])]
#[case::constants_only("(1 + 2)", &[])]
#[case::nested_func_typecast("LOWER(email::text)", &["email"])]
#[case::multi_arg_func("COALESCE(first_name, last_name, 'unknown')", &["first_name", "last_name"])]
#[case::bool_expr("(a > 0 AND b > 0)", &["a", "b"])]
#[case::case_with_default("(CASE WHEN status = 'active' THEN priority ELSE 0 END)", &["priority", "status"])]
#[case::case_simple_form("(CASE status WHEN 'a' THEN priority ELSE rank END)", &["priority", "rank", "status"])]
#[case::null_test_in_case("(CASE WHEN email IS NOT NULL THEN email ELSE 'none' END)", &["email"])]
#[case::greatest("GREATEST(a, b)", &["a", "b"])]
#[case::least("LEAST(x, y, z)", &["x", "y", "z"])]
#[case::dedup("(a + a)", &["a"])]
fn test_extract_column_refs(#[case] expr: &str, #[case] expected: &[&str]) {
    let expected: Vec<String> = expected.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(expr_index_refs(expr), expected);
}

#[rstest]
#[case::table("REINDEX TABLE orders;", ReindexObjectKind::Table, false)]
#[case::table_concurrently("REINDEX TABLE CONCURRENTLY orders;", ReindexObjectKind::Table, true)]
#[case::index("REINDEX INDEX idx_orders_status;", ReindexObjectKind::Index, false)]
#[case::index_concurrently(
    "REINDEX INDEX CONCURRENTLY idx_orders_status;",
    ReindexObjectKind::Index,
    true
)]
#[case::schema("REINDEX SCHEMA public;", ReindexObjectKind::Schema, false)]
#[case::schema_concurrently("REINDEX SCHEMA CONCURRENTLY public;", ReindexObjectKind::Schema, true)]
#[case::database("REINDEX DATABASE mydb;", ReindexObjectKind::Database, false)]
#[case::database_concurrently(
    "REINDEX DATABASE CONCURRENTLY mydb;",
    ReindexObjectKind::Database,
    true
)]
#[case::system("REINDEX SYSTEM mydb;", ReindexObjectKind::System, false)]
#[case::system_concurrently("REINDEX SYSTEM CONCURRENTLY mydb;", ReindexObjectKind::System, true)]
fn test_parse_reindex_kinds_and_concurrent(
    #[case] sql: &str,
    #[case] expected_kind: ReindexObjectKind,
    #[case] concurrent: bool,
) {
    let nodes = parse_sql(sql);
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::Reindex(r) => {
            assert_eq!(r.kind, expected_kind, "kind mismatch for: {sql}");
            assert_eq!(r.concurrent, concurrent, "concurrent mismatch for: {sql}");
        }
        other => panic!("Expected Reindex for `{sql}`, got: {other:?}"),
    }
}

#[test]
fn test_parse_reindex_table_relation_name() {
    let nodes = parse_sql("REINDEX TABLE myschema.orders;");
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::Reindex(r) => match &r.target {
            ReindexTarget::Relation(name) => assert_eq!(name.name, "orders"),
            other => panic!("Expected Relation target, got {:?}", other),
        },
        other => panic!("Expected Reindex, got: {:?}", other),
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
fn test_alter_index_attach_partition() {
    let sql = "ALTER INDEX idx_parent ATTACH PARTITION idx_child;";
    let nodes = parse_sql(sql);
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::AlterIndexAttachPartition {
            parent_index_name,
            child_index_name,
        } => {
            assert_eq!(parent_index_name, &QualifiedName::unqualified("idx_parent"));
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
            assert_eq!(
                parent_index_name,
                &QualifiedName::qualified("myschema", "idx_parent")
            );
            assert_eq!(child_index_name.name, "idx_child");
            assert_eq!(child_index_name.schema.as_deref(), Some("myschema"));
        }
        other => panic!("Expected AlterIndexAttachPartition, got {:?}", other),
    }
}

#[rstest]
#[case::default_btree("CREATE INDEX idx ON t (col);", "btree")]
#[case::gin("CREATE INDEX idx ON t USING gin (col);", "gin")]
#[case::gist("CREATE INDEX idx ON t USING gist (col);", "gist")]
#[case::hash("CREATE INDEX idx ON t USING hash (col);", "hash")]
#[case::brin("CREATE INDEX idx ON t USING brin (col);", "brin")]
fn test_parse_create_index_access_method(#[case] sql: &str, #[case] expected_method: &str) {
    let nodes = parse_sql(sql);
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::CreateIndex(ci) => {
            assert_eq!(ci.access_method, expected_method);
        }
        other => panic!("Expected CreateIndex, got: {:?}", other),
    }
}

// cases: with schemaname, without schemaname

#[test]
fn test_parse_alter_index_rename() {
    dbg!(parse_sql("CREATE INDEX foo ON bar (id);"));
    let nodes = parse_sql("ALTER INDEX schemaname.idx_foo RENAME TO idx_bar;");
    assert_eq!(nodes.len(), 1);
    match &nodes[0].node {
        IrNode::RenameIndex { index, new_name } => {
            assert_eq!(index.name, "idx_foo");
            assert_eq!(new_name, "idx_bar");
        }
        other => panic!("Expected RenameIndex, got: {:?}", other),
    }
}

#[rstest]
#[case::plain_column("a", "a")]
#[case::plain_column_desc_nulls_last("a DESC NULLS LAST", "a")]
#[case::plain_column_opclass("a text_pattern_ops", "a")]
#[case::plain_column_collate("a COLLATE \"C\"", "a")]
#[case::function_call("lower(email)", "lower")]
#[case::schema_qualified_function_call("pg_catalog.lower(email)", "lower")]
#[case::coalesce("coalesce(a, b)", "coalesce")]
#[case::greatest("greatest(a, b)", "greatest")]
#[case::least("least(a, b)", "least")]
#[case::nullif("nullif(a, b)", "nullif")]
#[case::array_constructor("(ARRAY[a, b])", "array")]
#[case::parenthesized_column("(a)", "a")]
#[case::subscript("(arr[1])", "arr")]
#[case::jsonb_subscript("(js['k'])", "js")]
#[case::field_selection("((p).x)", "x")]
#[case::operator("(a + b)", "expr")]
#[case::jsonb_operator("(js ->> 'k')", "expr")]
#[case::null_test_falls_back_to_expr("(a IS NULL)", "expr")]
#[case::bool_expr_falls_back_to_expr("(a > 0 AND b > 0)", "expr")]
#[case::cast_of_column("(a::text)", "a")]
#[case::cast_of_operator_to_text("((a + b)::text)", "text")]
#[case::cast_of_operator_to_bigint("(CAST(a + b AS bigint))", "int8")]
#[case::cast_of_operator_to_int("((js ->> 'k')::int)", "int4")]
#[case::cast_of_function_call("(lower(email)::text)", "lower")]
#[case::cast_of_coalesce("(coalesce(a, b)::text)", "coalesce")]
#[case::case_without_else("(CASE WHEN a > 0 THEN 1 END)", "case")]
#[case::case_else_column("(CASE WHEN a > 0 THEN 1 ELSE b END)", "b")]
#[case::case_else_function_call("(CASE WHEN a > 0 THEN 1 ELSE abs(b) END)", "abs")]
#[case::case_else_cast_of_operator("(CASE WHEN a > 0 THEN 1 ELSE (a + b)::int END)", "case")]
#[case::cast_of_case_without_else("((CASE WHEN a > 0 THEN 1 END)::text)", "text")]
#[case::collate_column("(email COLLATE \"C\")", "email")]
#[case::collate_function_call("(lower(email) COLLATE \"C\")", "lower")]
#[case::collate_cast_of_operator("((a + b)::text COLLATE \"C\")", "text")]
fn test_index_element_generated_name_part(#[case] element: &str, #[case] expected: &str) {
    let sql = format!("CREATE INDEX ON t ({element});");
    assert_eq!(generated_name_parts(&sql), [expected]);
}

#[test]
fn test_mixed_index_elements_each_contribute_a_name_part_in_order() {
    assert_eq!(
        generated_name_parts("CREATE INDEX ON i_mixed (lower(email), (a + b), b);"),
        ["lower", "expr", "b"]
    );
}

#[test]
fn test_repeated_index_elements_keep_unsuffixed_name_parts() {
    assert_eq!(
        generated_name_parts("CREATE INDEX ON i_repeat (lower(email), lower(email));"),
        ["lower", "lower"]
    );
}

#[test]
fn test_expression_element_stores_its_generated_name_part() {
    let ci = parsed_create_index("CREATE INDEX ON i_lower (lower(email));");
    assert_eq!(
        ci.columns,
        [IndexColumn::Expression {
            text: "lower(email)".to_string(),
            referenced_columns: vec!["email".to_string()],
            generated_name_part: "lower".to_string(),
        }]
    );
}

#[test]
fn test_create_index_include_columns() {
    let ci = parsed_create_index("CREATE INDEX ON i_include (a) INCLUDE (b, c);");
    assert_eq!(ci.columns, [IndexColumn::Column("a".to_string())]);
    assert_eq!(ci.include_columns, ["b", "c"]);
}

#[test]
fn test_create_index_without_include_has_no_include_columns() {
    let ci = parsed_create_index("CREATE INDEX ON i_single (a);");
    assert!(ci.include_columns.is_empty());
}
