//! pg_query spike - run with `cargo test --test pg_query_spike -- --nocapture`
//!
//! This test documents pg_query AST behavior for the Phase 1 agents.

// pg_query spike tests

/// Extract the canonical type name(s) from a CREATE TABLE statement
fn extract_type_info(sql: &str) -> String {
    let result = pg_query::parse(sql).expect("parse failed");
    let stmts = &result.protobuf.stmts;
    let stmt = stmts[0].stmt.as_ref().unwrap().node.as_ref().unwrap();

    if let pg_query::NodeEnum::CreateStmt(create) = stmt {
        let col_node = create.table_elts[0].node.as_ref().unwrap();
        if let pg_query::NodeEnum::ColumnDef(col) = col_node {
            let tn = col.type_name.as_ref().unwrap();
            let names: Vec<String> = tn
                .names
                .iter()
                .filter_map(|n| n.node.as_ref())
                .map(|n| match n {
                    pg_query::NodeEnum::String(s) => s.sval.clone(),
                    _ => "??".to_string(),
                })
                .collect();
            let typmods: Vec<String> = tn
                .typmods
                .iter()
                .filter_map(|n| n.node.as_ref())
                .map(|n| format!("{:?}", n))
                .collect();
            let constraints: Vec<String> = col
                .constraints
                .iter()
                .filter_map(|n| n.node.as_ref())
                .map(|n| format!("{:?}", n))
                .collect();
            let default = col
                .raw_default
                .as_ref()
                .map(|d| format!("{:?}", d.node.as_ref().unwrap()));

            format!(
                "names={}, typmods=[{}], typemod={}, is_not_null={}, constraints=[{}], raw_default={:?}",
                names.join("."),
                typmods.join(", "),
                tn.typemod,
                col.is_not_null,
                constraints.join(", "),
                default,
            )
        } else {
            "not a column".to_string()
        }
    } else {
        "not a create stmt".to_string()
    }
}

#[test]
fn spike_type_canonical_names() {
    let cases = [
        ("int", "CREATE TABLE t (col int);"),
        ("integer", "CREATE TABLE t (col integer);"),
        ("int4", "CREATE TABLE t (col int4);"),
        ("int8", "CREATE TABLE t (col int8);"),
        ("bigint", "CREATE TABLE t (col bigint);"),
        ("smallint", "CREATE TABLE t (col smallint);"),
        ("int2", "CREATE TABLE t (col int2);"),
        ("bool", "CREATE TABLE t (col bool);"),
        ("boolean", "CREATE TABLE t (col boolean);"),
        ("varchar", "CREATE TABLE t (col varchar);"),
        ("varchar(100)", "CREATE TABLE t (col varchar(100));"),
        (
            "character varying",
            "CREATE TABLE t (col character varying);",
        ),
        (
            "character varying(100)",
            "CREATE TABLE t (col character varying(100));",
        ),
        ("text", "CREATE TABLE t (col text);"),
        ("char", "CREATE TABLE t (col char);"),
        ("char(5)", "CREATE TABLE t (col char(5));"),
        ("character", "CREATE TABLE t (col character);"),
        ("serial", "CREATE TABLE t (col serial);"),
        ("bigserial", "CREATE TABLE t (col bigserial);"),
        ("numeric", "CREATE TABLE t (col numeric);"),
        ("numeric(10,2)", "CREATE TABLE t (col numeric(10,2));"),
        ("decimal", "CREATE TABLE t (col decimal);"),
        ("float", "CREATE TABLE t (col float);"),
        ("real", "CREATE TABLE t (col real);"),
        ("double precision", "CREATE TABLE t (col double precision);"),
        ("timestamp", "CREATE TABLE t (col timestamp);"),
        ("timestamptz", "CREATE TABLE t (col timestamptz);"),
        (
            "timestamp with time zone",
            "CREATE TABLE t (col timestamp with time zone);",
        ),
        ("uuid", "CREATE TABLE t (col uuid);"),
        ("jsonb", "CREATE TABLE t (col jsonb);"),
        ("json", "CREATE TABLE t (col json);"),
    ];

    println!("\n{:<30} | CANONICAL FORM", "INPUT TYPE");
    println!("{:-<30}-+-{:-<80}", "", "");

    for (label, sql) in cases {
        let info = extract_type_info(sql);
        println!("{:<30} | {}", label, info);
    }
}

#[test]
fn spike_serial_expansion() {
    println!("\n=== serial ===");
    println!("{}", extract_type_info("CREATE TABLE t (id serial);"));
    println!("\n=== bigserial ===");
    println!("{}", extract_type_info("CREATE TABLE t (id bigserial);"));
    println!("\n=== serial PRIMARY KEY ===");
    println!(
        "{}",
        extract_type_info("CREATE TABLE t (id serial PRIMARY KEY);")
    );
}

/// Extract constraint info from CREATE TABLE to compare inline vs table-level
fn extract_create_info(sql: &str) -> String {
    let result = pg_query::parse(sql).expect("parse failed");
    let stmts = &result.protobuf.stmts;
    let stmt = stmts[0].stmt.as_ref().unwrap().node.as_ref().unwrap();

    if let pg_query::NodeEnum::CreateStmt(create) = stmt {
        let mut lines = vec![];

        for elt in &create.table_elts {
            let node = elt.node.as_ref().unwrap();
            match node {
                pg_query::NodeEnum::ColumnDef(col) => {
                    let constraint_types: Vec<String> = col
                        .constraints
                        .iter()
                        .filter_map(|c| c.node.as_ref())
                        .map(|c| match c {
                            pg_query::NodeEnum::Constraint(con) => {
                                format!("contype={:?}", con.contype())
                            }
                            _ => "??".to_string(),
                        })
                        .collect();
                    lines.push(format!(
                        "  Column '{}': constraints=[{}]",
                        col.colname,
                        constraint_types.join(", ")
                    ));
                }
                pg_query::NodeEnum::Constraint(con) => {
                    let keys: Vec<String> = con
                        .keys
                        .iter()
                        .filter_map(|k| k.node.as_ref())
                        .map(|k| match k {
                            pg_query::NodeEnum::String(s) => s.sval.clone(),
                            _ => "??".to_string(),
                        })
                        .collect();
                    let fk_info = if let Some(pk_table) = con.pktable.as_ref() {
                        let fk_cols: Vec<String> = con
                            .fk_attrs
                            .iter()
                            .filter_map(|k| k.node.as_ref())
                            .map(|k| match k {
                                pg_query::NodeEnum::String(s) => s.sval.clone(),
                                _ => "??".to_string(),
                            })
                            .collect();
                        let pk_cols: Vec<String> = con
                            .pk_attrs
                            .iter()
                            .filter_map(|k| k.node.as_ref())
                            .map(|k| match k {
                                pg_query::NodeEnum::String(s) => s.sval.clone(),
                                _ => "??".to_string(),
                            })
                            .collect();
                        format!(
                            ", fk_attrs=[{}], pk_table={}, pk_attrs=[{}]",
                            fk_cols.join(","),
                            pk_table.relname,
                            pk_cols.join(","),
                        )
                    } else {
                        String::new()
                    };
                    lines.push(format!(
                        "  TableConstraint: contype={:?}, keys=[{}]{}",
                        con.contype(),
                        keys.join(","),
                        fk_info,
                    ));
                }
                _ => lines.push(format!("  Other: {:?}", node)),
            }
        }
        lines.join("\n")
    } else {
        "not a create stmt".to_string()
    }
}

#[test]
fn spike_inline_vs_table_level_constraints() {
    let cases = [
        ("Inline PK", "CREATE TABLE foo (id int PRIMARY KEY);"),
        (
            "Table-level PK",
            "CREATE TABLE foo (id int, PRIMARY KEY (id));",
        ),
        (
            "Inline FK (REFERENCES)",
            "CREATE TABLE orders (customer_id int REFERENCES customers(id));",
        ),
        (
            "Table-level FK",
            "CREATE TABLE orders (customer_id int, FOREIGN KEY (customer_id) REFERENCES customers(id));",
        ),
        ("Inline UNIQUE", "CREATE TABLE users (email text UNIQUE);"),
        (
            "Table-level UNIQUE",
            "CREATE TABLE users (email text, UNIQUE (email));",
        ),
        ("Inline NOT NULL", "CREATE TABLE t (col text NOT NULL);"),
        ("Inline CHECK", "CREATE TABLE t (col int CHECK (col > 0));"),
        (
            "Table-level CHECK",
            "CREATE TABLE t (col int, CHECK (col > 0));",
        ),
    ];

    for (label, sql) in cases {
        println!("\n=== {} ===", label);
        println!("{}", extract_create_info(sql));
    }
}

#[test]
fn spike_defaults_ast() {
    let cases = [
        ("Literal 0", "CREATE TABLE t (col int DEFAULT 0);"),
        (
            "Literal string",
            "CREATE TABLE t (col text DEFAULT 'active');",
        ),
        ("now()", "CREATE TABLE t (col timestamp DEFAULT now());"),
        (
            "gen_random_uuid()",
            "CREATE TABLE t (col uuid DEFAULT gen_random_uuid());",
        ),
        (
            "nextval",
            "CREATE TABLE t (col int DEFAULT nextval('t_col_seq'::regclass));",
        ),
        ("TRUE", "CREATE TABLE t (col bool DEFAULT TRUE);"),
    ];

    for (label, sql) in cases {
        println!("\n=== {} ===", label);
        println!("{}", extract_type_info(sql));
    }
}

#[test]
fn spike_alter_table_ast() {
    let sqls = [
        "ALTER TABLE orders ADD COLUMN status text NOT NULL DEFAULT 'pending';",
        "ALTER TABLE orders DROP COLUMN old_field;",
        "ALTER TABLE orders ADD CONSTRAINT fk_customer FOREIGN KEY (customer_id) REFERENCES customers(id);",
        "ALTER TABLE orders ALTER COLUMN status TYPE varchar(100);",
        "ALTER TABLE orders ALTER COLUMN price SET NOT NULL;",
    ];

    for sql in sqls {
        let result = pg_query::parse(sql).expect("parse failed");
        let stmt = result.protobuf.stmts[0]
            .stmt
            .as_ref()
            .unwrap()
            .node
            .as_ref()
            .unwrap();
        println!("\n=== {} ===", sql);
        if let pg_query::NodeEnum::AlterTableStmt(alter) = stmt {
            println!("table: {}", alter.relation.as_ref().unwrap().relname);
            for cmd_node in &alter.cmds {
                if let Some(pg_query::NodeEnum::AlterTableCmd(cmd)) = cmd_node.node.as_ref() {
                    println!(
                        "  subtype={:?}, name='{}', behavior={:?}",
                        cmd.subtype(),
                        cmd.name,
                        cmd.behavior()
                    );
                    if let Some(ref def) = cmd.def {
                        println!("  def={:?}", def.node.as_ref().unwrap());
                    }
                    if let Some(ref new_owner) = cmd.newowner {
                        println!("  newowner={:?}", new_owner);
                    }
                }
            }
        }
    }
}

#[test]
fn spike_create_drop_index_ast() {
    let sqls = [
        "CREATE INDEX idx_status ON orders (status);",
        "CREATE INDEX CONCURRENTLY idx_status ON orders (status);",
        "CREATE UNIQUE INDEX idx_email ON users (email);",
        "CREATE INDEX idx_composite ON orders (customer_id, status);",
        "DROP INDEX idx_status;",
        "DROP INDEX CONCURRENTLY idx_status;",
        "DROP INDEX IF EXISTS idx_status;",
        "DROP INDEX CONCURRENTLY IF EXISTS idx_status;",
        "DROP TABLE orders;",
        "DROP TABLE IF EXISTS orders;",
    ];

    for sql in sqls {
        let result = pg_query::parse(sql).expect("parse failed");
        let stmt = result.protobuf.stmts[0]
            .stmt
            .as_ref()
            .unwrap()
            .node
            .as_ref()
            .unwrap();
        println!("\n=== {} ===", sql);
        match stmt {
            pg_query::NodeEnum::IndexStmt(idx) => {
                println!(
                    "  name={}, table={}, unique={}, concurrent={}",
                    idx.idxname,
                    idx.relation.as_ref().unwrap().relname,
                    idx.unique,
                    idx.concurrent,
                );
                for param in &idx.index_params {
                    if let Some(pg_query::NodeEnum::IndexElem(elem)) = param.node.as_ref() {
                        println!("  column: name={}", elem.name);
                    }
                }
            }
            pg_query::NodeEnum::DropStmt(drop) => {
                println!(
                    "  concurrent={}, missing_ok={}",
                    drop.concurrent, drop.missing_ok
                );
                for obj in &drop.objects {
                    println!("  object: {:?}", obj.node.as_ref().unwrap());
                }
            }
            _ => println!("  unexpected: {:?}", stmt),
        }
    }
}

#[test]
fn spike_do_block() {
    let sql = "DO $$ BEGIN RAISE NOTICE 'hello'; END $$;";
    let result = pg_query::parse(sql).expect("parse failed");
    let stmt = result.protobuf.stmts[0]
        .stmt
        .as_ref()
        .unwrap()
        .node
        .as_ref()
        .unwrap();
    println!("\n=== DO block ===");
    println!("{:#?}", stmt);
}

#[test]
fn spike_multi_statement_offsets() {
    let sql = "CREATE TABLE foo (id int);\nCREATE INDEX idx ON foo (id);\nALTER TABLE foo ADD COLUMN name text;";
    let result = pg_query::parse(sql).expect("parse failed");

    println!("\n=== Multi-statement offsets ===");
    for (i, stmt) in result.protobuf.stmts.iter().enumerate() {
        println!(
            "  stmt[{}]: location={}, len={}",
            i, stmt.stmt_location, stmt.stmt_len
        );
    }
}

#[test]
fn spike_ignored_statements() {
    let sqls = [
        "GRANT SELECT ON orders TO readonly;",
        "COMMENT ON TABLE orders IS 'Order table';",
        "CREATE VIEW order_view AS SELECT * FROM orders;",
        "CREATE FUNCTION my_func() RETURNS void AS $$ BEGIN END; $$ LANGUAGE plpgsql;",
    ];

    for sql in sqls {
        let result = pg_query::parse(sql);
        println!("\n=== {} ===", sql);
        match result {
            Ok(parsed) => {
                let stmt = parsed.protobuf.stmts[0]
                    .stmt
                    .as_ref()
                    .unwrap()
                    .node
                    .as_ref()
                    .unwrap();
                // Just print the variant name
                println!("  Parsed as: {:?}", std::mem::discriminant(stmt));
                println!("  Full: {:#?}", stmt);
            }
            Err(e) => println!("  Parse error: {}", e),
        }
    }
}

#[test]
fn spike_index_rangevar_inh() {
    // Verifies RangeVar.inh behavior for CREATE INDEX with/without ONLY.
    // Normal: inh = true (recurse into partitions).
    // ONLY:   inh = false (parent-only, no recursion).
    // pg_query explicitly sets inh=true for the normal case, so the protobuf
    // bool default of false does not cause ambiguity.
    let sqls = [
        ("CREATE INDEX idx ON foo (col)", true),
        ("CREATE INDEX idx ON ONLY foo (col)", false),
    ];
    for (sql, expected_inh) in sqls {
        let result = pg_query::parse(sql).expect("parse failed");
        let stmt = result.protobuf.stmts[0]
            .stmt
            .as_ref()
            .unwrap()
            .node
            .as_ref()
            .unwrap();
        if let pg_query::NodeEnum::IndexStmt(idx) = stmt {
            let rel = idx.relation.as_ref().unwrap();
            assert_eq!(rel.inh, expected_inh, "inh mismatch for: {}", sql);
        } else {
            panic!("expected IndexStmt for: {}", sql);
        }
    }
}

#[test]
fn spike_truncate_stmt() {
    let sqls = [
        "TRUNCATE TABLE audit_trail;",
        "TRUNCATE TABLE audit_trail CASCADE;",
        "TRUNCATE TABLE t1, t2, t3 CASCADE;",
    ];

    for sql in sqls {
        let result = pg_query::parse(sql).expect("parse failed");
        let stmt = result.protobuf.stmts[0]
            .stmt
            .as_ref()
            .unwrap()
            .node
            .as_ref()
            .unwrap();
        println!("\n=== {} ===", sql);
        println!("{:#?}", stmt);
    }
}

#[test]
fn spike_alter_index_attach_partition() {
    // Investigates how pg_query represents ALTER INDEX ... ATTACH PARTITION.
    // This is needed for partition-aware index tracking: when a child index is
    // attached to a parent ON ONLY index, we flip parent.only = false.
    let sqls = [
        // Basic ATTACH PARTITION
        "ALTER INDEX idx_parent ATTACH PARTITION idx_child;",
        // Schema-qualified
        "ALTER INDEX myschema.idx_parent ATTACH PARTITION myschema.idx_child;",
        // For comparison: ALTER TABLE ... ATTACH PARTITION (table-level)
        "ALTER TABLE parent_table ATTACH PARTITION child_table FOR VALUES FROM (1) TO (100);",
        // ALTER INDEX with other operations (SET/RESET)
        "ALTER INDEX idx_foo SET (fillfactor = 70);",
        // ALTER INDEX RENAME
        "ALTER INDEX idx_old RENAME TO idx_new;",
        // ALTER INDEX SET TABLESPACE
        "ALTER INDEX idx_foo SET TABLESPACE fast_ssd;",
        // ALTER INDEX ALL IN TABLESPACE
        "ALTER INDEX ALL IN TABLESPACE old_space SET TABLESPACE new_space;",
    ];

    for sql in sqls {
        let result = pg_query::parse(sql);
        println!("\n=== {} ===", sql);
        match result {
            Ok(parsed) => {
                let stmt = parsed.protobuf.stmts[0]
                    .stmt
                    .as_ref()
                    .unwrap()
                    .node
                    .as_ref()
                    .unwrap();
                // Print the discriminant to see which NodeEnum variant it is
                println!("  Variant: {:?}", std::mem::discriminant(stmt));
                println!("  Full AST:");
                println!("{:#?}", stmt);
            }
            Err(e) => println!("  Parse error: {}", e),
        }
    }
}

// ---------------------------------------------------------------------------
// DROP SCHEMA AST structure spike
// ---------------------------------------------------------------------------

#[test]
fn spike_drop_schema_ast() {
    let sql = "DROP SCHEMA myschema CASCADE";
    let result = pg_query::parse(sql).expect("parse failed");
    let stmt = result.protobuf.stmts[0]
        .stmt
        .as_ref()
        .unwrap()
        .node
        .as_ref()
        .unwrap();

    // Verify it's a DropStmt with ObjectSchema type
    if let pg_query::NodeEnum::DropStmt(drop) = stmt {
        assert_eq!(
            drop.remove_type(),
            pg_query::protobuf::ObjectType::ObjectSchema,
            "Expected ObjectSchema"
        );
        assert!(!drop.missing_ok, "Should not have IF EXISTS");
        assert_eq!(
            drop.behavior(),
            pg_query::protobuf::DropBehavior::DropCascade,
            "Expected CASCADE"
        );

        // Schema names are String nodes directly in objects[] (NOT wrapped in List)
        assert!(!drop.objects.is_empty(), "Should have at least one object");
        let first_obj = drop.objects[0].node.as_ref().unwrap();
        if let pg_query::NodeEnum::String(s) = first_obj {
            assert_eq!(s.sval, "myschema");
        } else {
            panic!("Expected String node for schema name, got: {:?}", first_obj);
        }
    } else {
        panic!("Expected DropStmt, got: {:?}", stmt);
    }
}

#[test]
fn spike_drop_schema_if_exists_no_cascade() {
    let sql = "DROP SCHEMA IF EXISTS myschema";
    let result = pg_query::parse(sql).expect("parse failed");
    let stmt = result.protobuf.stmts[0]
        .stmt
        .as_ref()
        .unwrap()
        .node
        .as_ref()
        .unwrap();

    if let pg_query::NodeEnum::DropStmt(drop) = stmt {
        assert_eq!(
            drop.remove_type(),
            pg_query::protobuf::ObjectType::ObjectSchema
        );
        assert!(drop.missing_ok, "Should have IF EXISTS");
        assert_eq!(
            drop.behavior(),
            pg_query::protobuf::DropBehavior::DropRestrict,
            "Expected RESTRICT (no CASCADE)"
        );

        let first_obj = drop.objects[0].node.as_ref().unwrap();
        if let pg_query::NodeEnum::String(s) = first_obj {
            assert_eq!(s.sval, "myschema");
        } else {
            panic!("Expected String node");
        }
    } else {
        panic!("Expected DropStmt");
    }
}

#[test]
fn spike_drop_schema_multiple_schemas() {
    let sql = "DROP SCHEMA foo, bar CASCADE";
    let result = pg_query::parse(sql).expect("parse failed");

    // Multiple schemas in one DROP produce a single DropStmt with multiple objects
    assert_eq!(
        result.protobuf.stmts.len(),
        1,
        "Should be a single statement"
    );

    let stmt = result.protobuf.stmts[0]
        .stmt
        .as_ref()
        .unwrap()
        .node
        .as_ref()
        .unwrap();

    if let pg_query::NodeEnum::DropStmt(drop) = stmt {
        assert_eq!(
            drop.remove_type(),
            pg_query::protobuf::ObjectType::ObjectSchema
        );
        assert_eq!(drop.objects.len(), 2, "Should have two schema objects");

        // Both are bare String nodes
        let names: Vec<&str> = drop
            .objects
            .iter()
            .filter_map(|obj| match obj.node.as_ref() {
                Some(pg_query::NodeEnum::String(s)) => Some(s.sval.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["foo", "bar"]);
    } else {
        panic!("Expected DropStmt, got: {:?}", stmt);
    }
}

// ---------------------------------------------------------------------------
// SET/DROP DEFAULT and index access method spikes
// ---------------------------------------------------------------------------

#[test]
fn spike_alter_column_set_drop_default() {
    let sqls = [
        "ALTER TABLE t ALTER COLUMN col SET DEFAULT 42;",
        "ALTER TABLE t ALTER COLUMN col SET DEFAULT now();",
        "ALTER TABLE t ALTER COLUMN col DROP DEFAULT;",
    ];
    for sql in sqls {
        let result = pg_query::parse(sql).expect("parse failed");
        let stmt = result.protobuf.stmts[0]
            .stmt
            .as_ref()
            .unwrap()
            .node
            .as_ref()
            .unwrap();
        println!("\n=== {} ===", sql);
        if let pg_query::NodeEnum::AlterTableStmt(alter) = stmt {
            for cmd_node in &alter.cmds {
                if let Some(pg_query::NodeEnum::AlterTableCmd(cmd)) = cmd_node.node.as_ref() {
                    println!(
                        "  subtype={:?}, name='{}', def={:?}",
                        cmd.subtype(),
                        cmd.name,
                        cmd.def.as_ref().map(|d| d.node.as_ref())
                    );
                }
            }
        }
    }
}

#[test]
fn spike_index_access_method() {
    let sqls = [
        "CREATE INDEX idx ON t (col);",
        "CREATE INDEX idx ON t USING btree (col);",
        "CREATE INDEX idx ON t USING gin (col);",
        "CREATE INDEX idx ON t USING gist (col);",
        "CREATE INDEX idx ON t USING hash (col);",
        "CREATE INDEX idx ON t USING brin (col);",
    ];
    for sql in sqls {
        let result = pg_query::parse(sql).expect("parse failed");
        let stmt = result.protobuf.stmts[0]
            .stmt
            .as_ref()
            .unwrap()
            .node
            .as_ref()
            .unwrap();
        if let pg_query::NodeEnum::IndexStmt(idx) = stmt {
            println!("{}: access_method={:?}", sql, idx.access_method);
        }
    }
}

// ---------------------------------------------------------------------------
// VACUUM statements
// ---------------------------------------------------------------------------

#[test]
fn spike_vacuum_full() {
    let result = pg_query::parse("VACUUM FULL orders;").expect("should parse");
    let stmt = result.protobuf.stmts[0]
        .stmt
        .as_ref()
        .unwrap()
        .node
        .as_ref()
        .unwrap();

    if let pg_query::NodeEnum::VacuumStmt(vacuum) = stmt {
        // FULL is indicated by a DefElem with defname "full" in options
        assert!(vacuum.is_vacuumcmd, "is_vacuumcmd is true even for FULL");
        assert_eq!(vacuum.options.len(), 1);
        let opt = match vacuum.options[0].node.as_ref().unwrap() {
            pg_query::NodeEnum::DefElem(d) => d,
            other => panic!("Expected DefElem, got: {other:?}"),
        };
        assert_eq!(opt.defname, "full");

        // Relation is wrapped in VacuumRelation
        assert_eq!(vacuum.rels.len(), 1);
        let rel = match vacuum.rels[0].node.as_ref().unwrap() {
            pg_query::NodeEnum::VacuumRelation(vr) => vr.relation.as_ref().unwrap(),
            other => panic!("Expected VacuumRelation, got: {other:?}"),
        };
        assert_eq!(rel.relname, "orders");
        assert_eq!(rel.schemaname, "");
    } else {
        panic!("Expected VacuumStmt, got: {stmt:?}");
    }
}

#[test]
fn spike_vacuum_plain() {
    let result = pg_query::parse("VACUUM orders;").expect("should parse");
    let stmt = result.protobuf.stmts[0]
        .stmt
        .as_ref()
        .unwrap()
        .node
        .as_ref()
        .unwrap();

    if let pg_query::NodeEnum::VacuumStmt(vacuum) = stmt {
        // Plain VACUUM has no options
        assert!(vacuum.is_vacuumcmd);
        assert!(vacuum.options.is_empty(), "Plain VACUUM has no options");
        assert_eq!(vacuum.rels.len(), 1);
    } else {
        panic!("Expected VacuumStmt, got: {stmt:?}");
    }
}

#[test]
fn spike_vacuum_full_analyze() {
    let result = pg_query::parse("VACUUM (FULL, ANALYZE) orders;").expect("should parse");
    let stmt = result.protobuf.stmts[0]
        .stmt
        .as_ref()
        .unwrap()
        .node
        .as_ref()
        .unwrap();

    if let pg_query::NodeEnum::VacuumStmt(vacuum) = stmt {
        // Parenthesized form with FULL and ANALYZE
        assert_eq!(vacuum.options.len(), 2);
        let option_names: Vec<&str> = vacuum
            .options
            .iter()
            .filter_map(|n| match n.node.as_ref() {
                Some(pg_query::NodeEnum::DefElem(d)) => Some(d.defname.as_str()),
                _ => None,
            })
            .collect();
        assert!(option_names.contains(&"full"));
        assert!(option_names.contains(&"analyze"));
    } else {
        panic!("Expected VacuumStmt, got: {stmt:?}");
    }
}

// ---------------------------------------------------------------------------
// SQL keyword defaults vs function call defaults
// ---------------------------------------------------------------------------

/// Documents how pg_query represents SQL keyword defaults (CURRENT_TIMESTAMP,
/// CURRENT_DATE, etc.) versus function call defaults (now(), random()).
///
/// Key finding: SQL-standard datetime keywords like CURRENT_TIMESTAMP are NOT
/// placed in `raw_default` at all. Instead, pg_query puts them inside a
/// `Constraint` node with `contype = Default`. The constraint's `raw_expr`
/// contains an `SQLValueFunction` node. Only actual function call syntax
/// (e.g. `now()`) produces a `FuncCall` node in `raw_default`.
///
/// This means our `convert_default_expr` never sees SQL keywords — they arrive
/// via the constraint path, not the raw_default path. The practical effect is
/// that `CURRENT_TIMESTAMP`, `CURRENT_DATE`, etc. become `DefaultExpr::Other`
/// and never reach `classify_volatility()`.
#[test]
fn spike_sql_value_function_vs_func_call() {
    // SQL keywords: pg_query puts DEFAULT in a Constraint node, not raw_default.
    // The default value is in Constraint.raw_expr as SQLValueFunction.
    let keyword_sql = "CREATE TABLE t (col timestamptz DEFAULT CURRENT_TIMESTAMP);";
    let result = pg_query::parse(keyword_sql).expect("parse failed");
    let stmt = result.protobuf.stmts[0]
        .stmt
        .as_ref()
        .unwrap()
        .node
        .as_ref()
        .unwrap();
    let pg_query::NodeEnum::CreateStmt(create) = stmt else {
        panic!("expected CreateStmt");
    };
    let col_node = create.table_elts[0].node.as_ref().unwrap();
    let pg_query::NodeEnum::ColumnDef(col) = col_node else {
        panic!("expected ColumnDef");
    };

    // raw_default is None for SQL keyword defaults
    assert!(
        col.raw_default.is_none(),
        "CURRENT_TIMESTAMP should NOT appear in raw_default"
    );

    // Instead it's in a Constraint node with contype = Default
    let constraint = col
        .constraints
        .iter()
        .find_map(|c| match c.node.as_ref() {
            Some(pg_query::NodeEnum::Constraint(con))
                if con.contype() == pg_query::protobuf::ConstrType::ConstrDefault =>
            {
                Some(con)
            }
            _ => None,
        })
        .expect("should have a DEFAULT constraint");

    let raw_expr = constraint
        .raw_expr
        .as_ref()
        .expect("DEFAULT constraint should have raw_expr")
        .node
        .as_ref()
        .unwrap();

    assert!(
        matches!(raw_expr, pg_query::NodeEnum::SqlvalueFunction(_)),
        "CURRENT_TIMESTAMP should be SQLValueFunction, got: {raw_expr:?}"
    );

    // Function calls: pg_query puts these in raw_default as FuncCall.
    let func_sql = "CREATE TABLE t (col timestamptz DEFAULT now());";
    let result = pg_query::parse(func_sql).expect("parse failed");
    let stmt = result.protobuf.stmts[0]
        .stmt
        .as_ref()
        .unwrap()
        .node
        .as_ref()
        .unwrap();
    let pg_query::NodeEnum::CreateStmt(create) = stmt else {
        panic!("expected CreateStmt");
    };
    let col_node = create.table_elts[0].node.as_ref().unwrap();
    let pg_query::NodeEnum::ColumnDef(col) = col_node else {
        panic!("expected ColumnDef");
    };

    // For function calls, the default CAN appear in either raw_default or constraints.
    // Check both paths.
    let func_node = if let Some(ref rd) = col.raw_default {
        rd.node.clone().unwrap()
    } else {
        // Fall back to constraint path
        let constraint = col
            .constraints
            .iter()
            .find_map(|c| match c.node.as_ref() {
                Some(pg_query::NodeEnum::Constraint(con))
                    if con.contype() == pg_query::protobuf::ConstrType::ConstrDefault =>
                {
                    Some(con)
                }
                _ => None,
            })
            .expect("should have a DEFAULT constraint");
        constraint
            .raw_expr
            .as_ref()
            .expect("should have raw_expr")
            .node
            .clone()
            .unwrap()
    };

    assert!(
        matches!(func_node, pg_query::NodeEnum::FuncCall(_)),
        "now() should be FuncCall, got: {func_node:?}"
    );

    // Verify all SQL datetime keywords go through the constraint path
    let keyword_cases = [
        (
            "CURRENT_TIMESTAMP",
            "CREATE TABLE t (c timestamptz DEFAULT CURRENT_TIMESTAMP);",
        ),
        (
            "CURRENT_DATE",
            "CREATE TABLE t (c date DEFAULT CURRENT_DATE);",
        ),
        (
            "CURRENT_TIME",
            "CREATE TABLE t (c timetz DEFAULT CURRENT_TIME);",
        ),
        ("LOCALTIME", "CREATE TABLE t (c time DEFAULT LOCALTIME);"),
        (
            "LOCALTIMESTAMP",
            "CREATE TABLE t (c timestamp DEFAULT LOCALTIMESTAMP);",
        ),
    ];

    for (label, sql) in keyword_cases {
        let result = pg_query::parse(sql).expect("parse failed");
        let stmt = result.protobuf.stmts[0]
            .stmt
            .as_ref()
            .unwrap()
            .node
            .as_ref()
            .unwrap();
        let pg_query::NodeEnum::CreateStmt(create) = stmt else {
            panic!("expected CreateStmt");
        };
        let col_node = create.table_elts[0].node.as_ref().unwrap();
        let pg_query::NodeEnum::ColumnDef(col) = col_node else {
            panic!("expected ColumnDef");
        };
        assert!(
            col.raw_default.is_none(),
            "{label} should NOT be in raw_default"
        );
    }
}
