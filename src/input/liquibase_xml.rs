//! Lightweight XML fallback parser for Liquibase changelogs
//!
//! Used when Java/Liquibase binary is unavailable. Parses Liquibase XML
//! changelog files directly using `quick-xml` and generates SQL statements
//! from common change types.
//!
//! Supported change types:
//! - `<sql>` - raw SQL content
//! - `<createTable>` - generates CREATE TABLE SQL
//! - `<addColumn>` - generates ALTER TABLE ADD COLUMN SQL
//! - `<dropColumn>` - generates ALTER TABLE DROP COLUMN SQL (single and multi-column)
//! - `<createIndex>` - generates CREATE INDEX SQL
//! - `<dropTable>` - generates DROP TABLE SQL
//! - `<dropIndex>` - generates DROP INDEX SQL
//! - `<addForeignKeyConstraint>` - generates ALTER TABLE ADD CONSTRAINT SQL
//! - `<addPrimaryKey>` - generates ALTER TABLE ADD CONSTRAINT ... PRIMARY KEY SQL
//! - `<addUniqueConstraint>` - generates ALTER TABLE ADD CONSTRAINT ... UNIQUE SQL
//! - `<modifyDataType>` - generates ALTER TABLE ALTER COLUMN TYPE SQL
//! - `<addNotNullConstraint>` - generates ALTER TABLE ALTER COLUMN SET NOT NULL SQL
//! - `<dropNotNullConstraint>` - generates ALTER TABLE ALTER COLUMN DROP NOT NULL SQL
//! - `<renameColumn>` - generates ALTER TABLE RENAME COLUMN SQL
//! - `<dropForeignKeyConstraint>` - generates ALTER TABLE DROP CONSTRAINT SQL
//! - `<dropPrimaryKey>` - generates ALTER TABLE DROP CONSTRAINT SQL
//! - `<dropUniqueConstraint>` - generates ALTER TABLE DROP CONSTRAINT SQL
//! - `<renameTable>` - generates ALTER TABLE RENAME TO SQL
//!
//! Supports `<include>` and `<includeAll>` for loading changelogs from
//! referenced files and directories.
//!
//! All identifiers (table, column, index, constraint names) are quoted with
//! double quotes in generated SQL to handle reserved words safely.
//!
//! Unknown change types are skipped with a SQL comment indicating they were
//! not processed, so the catalog can be flagged as potentially incomplete.

use crate::input::{LoadError, RawMigrationUnit};
use quick_xml::Reader;
use quick_xml::events::Event;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Lightweight XML fallback loader for Liquibase changelogs.
///
/// Parses common Liquibase change types directly from XML and generates
/// equivalent SQL statements. This is used when neither the bridge JAR
/// nor the Liquibase binary is available.
pub struct XmlFallbackLoader;

impl XmlFallbackLoader {
    /// Load migration units from a Liquibase XML changelog.
    ///
    /// Parses the XML file and converts each `<changeSet>` element into
    /// a `RawMigrationUnit` with generated SQL.
    pub fn load(&self, path: &Path) -> Result<Vec<RawMigrationUnit>, LoadError> {
        let root_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let mut visited = HashSet::new();
        self.load_with_root(path, root_dir, &mut visited)
    }

    /// Load a changelog, resolving `<include file="...">` paths relative
    /// to `root_dir` (the directory of the top-level changelog). Liquibase
    /// resolves include paths relative to the classpath root, which in
    /// practice is the directory containing the master changelog.
    fn load_with_root(
        &self,
        path: &Path,
        root_dir: &Path,
        visited: &mut HashSet<PathBuf>,
    ) -> Result<Vec<RawMigrationUnit>, LoadError> {
        // Skip files already processed (Liquibase silently ignores
        // duplicate includes). This also prevents true circular includes
        // from looping infinitely.
        let canonical = path.to_path_buf();
        if !visited.insert(canonical) {
            return Ok(Vec::new());
        }

        let content = std::fs::read_to_string(path).map_err(|e| LoadError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;

        // Liquibase changelogs can include .sql files that use the
        // "--liquibase formatted sql" format. Route these to a dedicated
        // parser instead of the XML parser.
        let is_sql = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("sql"));
        if is_sql {
            return parse_liquibase_formatted_sql(&content, path);
        }

        let mut units = parse_changelog_xml(&content, path)?;

        // Process <include> directives by loading referenced files.
        // Paths are resolved relative to root_dir (classpath root).
        let include_paths = extract_include_paths(&content, root_dir, path)?;
        for include_path in include_paths {
            let included_units = self.load_with_root(&include_path, root_dir, visited)?;
            units.extend(included_units);
        }

        Ok(units)
    }
}

/// State machine for tracking parser position within the XML document.
#[derive(Debug)]
enum ParseState {
    /// Outside any changeSet element.
    Root,
    /// Inside a <changeSet> element, collecting change types.
    InChangeSet(ChangeSetInfo),
    /// Inside a <sql> element within a changeSet, collecting text.
    InSqlTag(ChangeSetInfo, String),
    /// Inside a <createTable> element, collecting columns.
    InCreateTable(ChangeSetInfo, CreateTableState),
    /// Inside a <column> element within <createTable>.
    InCreateTableColumn(ChangeSetInfo, CreateTableState, ColumnState),
    /// Inside a <addColumn> element, collecting columns.
    InAddColumn(ChangeSetInfo, AddColumnState),
    /// Inside a <column> element within <addColumn>.
    InAddColumnColumn(ChangeSetInfo, AddColumnState, ColumnState),
    /// Inside a <createIndex> element, collecting columns.
    InCreateIndex(ChangeSetInfo, CreateIndexState),
    /// Inside a <dropColumn> element (multi-column form), collecting columns.
    InDropColumn(ChangeSetInfo, DropColumnState),
}

/// Information about the current changeSet being parsed.
#[derive(Debug, Clone)]
struct ChangeSetInfo {
    id: String,
    run_in_transaction: bool,
    line: usize,
    sql_parts: Vec<String>,
}

impl ChangeSetInfo {
    /// Create a new ChangeSetInfo from attributes of a <changeSet> element.
    fn from_attributes(attrs: &[(String, String)], line: usize) -> Self {
        let id = get_attr(attrs, "id").unwrap_or_default();
        let run_in_transaction = get_attr(attrs, "runInTransaction")
            .map(|v| v != "false")
            .unwrap_or(true);

        Self {
            id,
            run_in_transaction,
            line,
            sql_parts: Vec::new(),
        }
    }
}

/// State for parsing a <createTable> element.
#[derive(Debug, Clone)]
struct CreateTableState {
    table_name: String,
    schema_name: Option<String>,
    columns: Vec<ColumnDef>,
}

/// State for parsing an <addColumn> element.
#[derive(Debug, Clone)]
struct AddColumnState {
    table_name: String,
    schema_name: Option<String>,
    columns: Vec<ColumnDef>,
}

/// State for parsing a <createIndex> element.
#[derive(Debug, Clone)]
struct CreateIndexState {
    index_name: String,
    table_name: String,
    schema_name: Option<String>,
    unique: bool,
    columns: Vec<String>,
}

/// State for parsing a <dropColumn> element (multi-column form).
#[derive(Debug, Clone)]
struct DropColumnState {
    table_name: String,
    schema_name: Option<String>,
    columns: Vec<String>,
}

/// Temporary state for parsing a <column> element.
#[derive(Debug, Clone)]
struct ColumnState {
    name: String,
    type_name: String,
    constraints: ColumnConstraints,
    default_value: Option<String>,
}

/// Parsed column definition for SQL generation.
#[derive(Debug, Clone)]
struct ColumnDef {
    name: String,
    type_name: String,
    nullable: bool,
    primary_key: bool,
    unique: bool,
    default_value: Option<String>,
    foreign_key_name: Option<String>,
    referenced_table: Option<String>,
    referenced_schema: Option<String>,
    referenced_columns: Option<String>,
}

/// Constraints parsed from a <constraints> element within a column.
#[derive(Debug, Clone)]
struct ColumnConstraints {
    nullable: bool,
    primary_key: bool,
    unique: bool,
    foreign_key_name: Option<String>,
    referenced_table: Option<String>,
    referenced_schema: Option<String>,
    referenced_columns: Option<String>,
}

impl Default for ColumnConstraints {
    /// Default constraints: nullable=true (standard SQL default), no PK, no unique, no FK.
    fn default() -> Self {
        Self {
            nullable: true,
            primary_key: false,
            unique: false,
            foreign_key_name: None,
            referenced_table: None,
            referenced_schema: None,
            referenced_columns: None,
        }
    }
}

/// Parse a `--liquibase formatted sql` file into `RawMigrationUnit`s.
///
/// Each `--changeset author:id` comment starts a new changeset. All SQL
/// lines between changeset markers (or until EOF) form that changeset's SQL.
fn parse_liquibase_formatted_sql(
    content: &str,
    source_path: &Path,
) -> Result<Vec<RawMigrationUnit>, LoadError> {
    let mut units = Vec::new();
    let mut current_id: Option<String> = None;
    let mut current_sql = String::new();
    let mut current_line_offset: usize = 0;
    let mut run_in_transaction = true;

    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Skip the header line
        if trimmed.eq_ignore_ascii_case("--liquibase formatted sql") {
            continue;
        }

        // Check for changeset marker: --changeset author:id
        if let Some(rest) = trimmed
            .strip_prefix("--changeset ")
            .or_else(|| trimmed.strip_prefix("--changeSet "))
        {
            // Flush previous changeset
            if let Some(id) = current_id.take() {
                let sql = current_sql.trim().to_string();
                if !sql.is_empty() {
                    units.push(RawMigrationUnit {
                        id,
                        sql,
                        source_file: source_path.to_path_buf(),
                        source_line_offset: current_line_offset,
                        run_in_transaction,
                        is_down: false,
                    });
                }
            }

            // Parse "author:id [attributes...]" — the id is between the
            // first ':' and the next whitespace (attributes follow after).
            current_id = Some(
                rest.split_once(':')
                    .map(|(_, after_colon)| {
                        after_colon
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .to_string()
                    })
                    .unwrap_or_else(|| rest.trim().to_string()),
            );
            current_sql = String::new();
            current_line_offset = line_idx + 1; // 1-based
            run_in_transaction = !rest.contains("runInTransaction:false");
            continue;
        }

        // Skip other Liquibase comment directives
        if trimmed.starts_with("--rollback ")
            || trimmed.starts_with("--preconditions ")
            || trimmed.starts_with("--comment ")
        {
            continue;
        }

        // Accumulate SQL lines
        if current_id.is_some() {
            if !current_sql.is_empty() {
                current_sql.push('\n');
            }
            current_sql.push_str(line);
        }
    }

    // Flush last changeset
    if let Some(id) = current_id {
        let sql = current_sql.trim().to_string();
        if !sql.is_empty() {
            units.push(RawMigrationUnit {
                id,
                sql,
                source_file: source_path.to_path_buf(),
                source_line_offset: current_line_offset,
                run_in_transaction,
                is_down: false,
            });
        }
    }

    Ok(units)
}

///
/// Iterates through XML events using a state machine to track position
/// within the document structure. Each `<changeSet>` produces one
/// `RawMigrationUnit` with SQL generated from its child change elements.
fn parse_changelog_xml(xml: &str, source_path: &Path) -> Result<Vec<RawMigrationUnit>, LoadError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text_start = true;
    reader.config_mut().trim_text_end = true;

    let mut units: Vec<RawMigrationUnit> = Vec::new();
    let mut state = ParseState::Root;
    let mut buf = Vec::new();

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| LoadError::Parse {
                path: source_path.to_path_buf(),
                message: format!(
                    "XML parse error at position {}: {}",
                    reader.buffer_position(),
                    e
                ),
            })?;

        match event {
            Event::Eof => break,
            Event::Start(ref e) | Event::Empty(ref e) => {
                let tag_name = local_name_str(e.name().as_ref());
                let attrs = collect_attributes(e)?;
                let is_empty = matches!(event, Event::Empty(_));
                let current_line = byte_offset_to_line(xml, reader.buffer_position() as usize);

                state = handle_start_tag(
                    state,
                    &tag_name,
                    &attrs,
                    is_empty,
                    current_line,
                    source_path,
                )?;

                // If this was a self-closing tag, also handle the "end" transition
                if is_empty {
                    state = handle_end_tag(state, &tag_name, source_path, &mut units)?;
                }
            }
            Event::End(ref e) => {
                let tag_name = local_name_str(e.name().as_ref());
                state = handle_end_tag(state, &tag_name, source_path, &mut units)?;
            }
            Event::Text(ref e) => {
                let text = String::from_utf8_lossy(e.as_ref());
                state = handle_text(state, &text);
            }
            Event::CData(ref e) => {
                let text = String::from_utf8_lossy(e.as_ref());
                state = handle_text(state, &text);
            }
            // Ignore comments, processing instructions, etc.
            _ => {}
        }

        buf.clear();
    }

    Ok(units)
}

/// Handle an opening (or self-closing) XML tag, transitioning the state machine.
fn handle_start_tag(
    state: ParseState,
    tag_name: &str,
    attrs: &[(String, String)],
    is_empty: bool,
    line: usize,
    _source_path: &Path,
) -> Result<ParseState, LoadError> {
    match state {
        ParseState::Root => {
            if tag_name == "changeSet" {
                Ok(ParseState::InChangeSet(ChangeSetInfo::from_attributes(
                    attrs, line,
                )))
            } else {
                // Stay at root for databaseChangeLog, include, etc.
                Ok(ParseState::Root)
            }
        }
        ParseState::InChangeSet(mut cs) => {
            match tag_name {
                "sql" => Ok(ParseState::InSqlTag(cs, String::new())),
                "createTable" => {
                    let Some(table_name) = get_attr(attrs, "tableName") else {
                        eprintln!(
                            "Warning: <createTable> missing required 'tableName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let schema_name = get_attr(attrs, "schemaName");
                    Ok(ParseState::InCreateTable(
                        cs,
                        CreateTableState {
                            table_name,
                            schema_name,
                            columns: Vec::new(),
                        },
                    ))
                }
                "addColumn" => {
                    let Some(table_name) = get_attr(attrs, "tableName") else {
                        eprintln!(
                            "Warning: <addColumn> missing required 'tableName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let schema_name = get_attr(attrs, "schemaName");
                    Ok(ParseState::InAddColumn(
                        cs,
                        AddColumnState {
                            table_name,
                            schema_name,
                            columns: Vec::new(),
                        },
                    ))
                }
                "createIndex" => {
                    let index_name = get_attr(attrs, "indexName").unwrap_or_default();
                    let Some(table_name) = get_attr(attrs, "tableName") else {
                        eprintln!(
                            "Warning: <createIndex> missing required 'tableName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let schema_name = get_attr(attrs, "schemaName");
                    let unique = get_attr(attrs, "unique")
                        .map(|v| v == "true")
                        .unwrap_or(false);
                    Ok(ParseState::InCreateIndex(
                        cs,
                        CreateIndexState {
                            index_name,
                            table_name,
                            schema_name,
                            unique,
                            columns: Vec::new(),
                        },
                    ))
                }
                "dropTable" => {
                    let Some(table_name) = get_attr(attrs, "tableName") else {
                        eprintln!(
                            "Warning: <dropTable> missing required 'tableName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let schema_name = get_attr(attrs, "schemaName");
                    let qualified = qualify_name(&schema_name, &table_name);
                    cs.sql_parts.push(format!("DROP TABLE {};", qualified));
                    Ok(ParseState::InChangeSet(cs))
                }
                "dropIndex" => {
                    let Some(index_name) = get_attr(attrs, "indexName") else {
                        eprintln!(
                            "Warning: <dropIndex> missing required 'indexName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let schema_name = get_attr(attrs, "schemaName");
                    let qualified = qualify_name(&schema_name, &index_name);
                    cs.sql_parts.push(format!("DROP INDEX {};", qualified));
                    Ok(ParseState::InChangeSet(cs))
                }
                "addForeignKeyConstraint" => {
                    let Some(base_table) = get_attr(attrs, "baseTableName") else {
                        eprintln!(
                            "Warning: <addForeignKeyConstraint> missing required 'baseTableName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let Some(ref_table) = get_attr(attrs, "referencedTableName") else {
                        eprintln!(
                            "Warning: <addForeignKeyConstraint> missing required 'referencedTableName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let Some(base_columns) = get_attr(attrs, "baseColumnNames") else {
                        eprintln!(
                            "Warning: <addForeignKeyConstraint> missing required 'baseColumnNames' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let Some(ref_columns) = get_attr(attrs, "referencedColumnNames") else {
                        eprintln!(
                            "Warning: <addForeignKeyConstraint> missing required 'referencedColumnNames' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let sql = generate_add_fk_sql(
                        attrs,
                        &base_table,
                        &base_columns,
                        &ref_table,
                        &ref_columns,
                    );
                    cs.sql_parts.push(sql);
                    Ok(ParseState::InChangeSet(cs))
                }
                "addPrimaryKey" => {
                    let Some(table_name) = get_attr(attrs, "tableName") else {
                        eprintln!(
                            "Warning: <addPrimaryKey> missing required 'tableName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let Some(column_names) = get_attr(attrs, "columnNames") else {
                        eprintln!(
                            "Warning: <addPrimaryKey> missing required 'columnNames' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let sql = generate_add_pk_sql(attrs, &table_name, &column_names);
                    cs.sql_parts.push(sql);
                    Ok(ParseState::InChangeSet(cs))
                }
                "addUniqueConstraint" => {
                    let Some(table_name) = get_attr(attrs, "tableName") else {
                        eprintln!(
                            "Warning: <addUniqueConstraint> missing required 'tableName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let Some(column_names) = get_attr(attrs, "columnNames") else {
                        eprintln!(
                            "Warning: <addUniqueConstraint> missing required 'columnNames' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let sql = generate_add_unique_sql(attrs, &table_name, &column_names);
                    cs.sql_parts.push(sql);
                    Ok(ParseState::InChangeSet(cs))
                }
                "dropColumn" => {
                    let Some(table_name) = get_attr(attrs, "tableName") else {
                        eprintln!(
                            "Warning: <dropColumn> missing required 'tableName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let schema_name = get_attr(attrs, "schemaName");

                    // Single-column form: columnName on parent element
                    if let Some(column_name) = get_attr(attrs, "columnName") {
                        let qualified = qualify_name(&schema_name, &table_name);
                        cs.sql_parts.push(format!(
                            "ALTER TABLE {} DROP COLUMN {};",
                            qualified,
                            quote_ident(&column_name)
                        ));
                        Ok(ParseState::InChangeSet(cs))
                    } else if is_empty {
                        eprintln!(
                            "Warning: <dropColumn> has no columnName and no child elements, skipping"
                        );
                        Ok(ParseState::InChangeSet(cs))
                    } else {
                        // Multi-column form: collect from child <column> elements
                        Ok(ParseState::InDropColumn(
                            cs,
                            DropColumnState {
                                table_name,
                                schema_name,
                                columns: Vec::new(),
                            },
                        ))
                    }
                }
                "modifyDataType" => {
                    let Some(table_name) = get_attr(attrs, "tableName") else {
                        eprintln!(
                            "Warning: <modifyDataType> missing required 'tableName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let Some(column_name) = get_attr(attrs, "columnName") else {
                        eprintln!(
                            "Warning: <modifyDataType> missing required 'columnName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let Some(raw_data_type) = get_attr(attrs, "newDataType") else {
                        eprintln!(
                            "Warning: <modifyDataType> missing required 'newDataType' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let new_data_type = map_liquibase_type(&raw_data_type);
                    let schema_name = get_attr(attrs, "schemaName");
                    let qualified = qualify_name(&schema_name, &table_name);
                    cs.sql_parts.push(format!(
                        "ALTER TABLE {} ALTER COLUMN {} TYPE {};",
                        qualified,
                        quote_ident(&column_name),
                        new_data_type
                    ));
                    Ok(ParseState::InChangeSet(cs))
                }
                "addNotNullConstraint" => {
                    let Some(table_name) = get_attr(attrs, "tableName") else {
                        eprintln!("Warning: <addNotNullConstraint> missing 'tableName', skipping");
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let Some(column_name) = get_attr(attrs, "columnName") else {
                        eprintln!("Warning: <addNotNullConstraint> missing 'columnName', skipping");
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let schema_name = get_attr(attrs, "schemaName");
                    let qualified = qualify_name(&schema_name, &table_name);
                    cs.sql_parts.push(format!(
                        "ALTER TABLE {} ALTER COLUMN {} SET NOT NULL;",
                        qualified,
                        quote_ident(&column_name)
                    ));
                    Ok(ParseState::InChangeSet(cs))
                }
                "dropNotNullConstraint" => {
                    let Some(table_name) = get_attr(attrs, "tableName") else {
                        eprintln!("Warning: <dropNotNullConstraint> missing 'tableName', skipping");
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let Some(column_name) = get_attr(attrs, "columnName") else {
                        eprintln!(
                            "Warning: <dropNotNullConstraint> missing 'columnName', skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let schema_name = get_attr(attrs, "schemaName");
                    let qualified = qualify_name(&schema_name, &table_name);
                    cs.sql_parts.push(format!(
                        "ALTER TABLE {} ALTER COLUMN {} DROP NOT NULL;",
                        qualified,
                        quote_ident(&column_name)
                    ));
                    Ok(ParseState::InChangeSet(cs))
                }
                "renameColumn" => {
                    let Some(table_name) = get_attr(attrs, "tableName") else {
                        eprintln!(
                            "Warning: <renameColumn> missing required 'tableName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let Some(old_column) = get_attr(attrs, "oldColumnName") else {
                        eprintln!(
                            "Warning: <renameColumn> missing required 'oldColumnName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let Some(new_column) = get_attr(attrs, "newColumnName") else {
                        eprintln!(
                            "Warning: <renameColumn> missing required 'newColumnName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let schema_name = get_attr(attrs, "schemaName");
                    let qualified = qualify_name(&schema_name, &table_name);
                    cs.sql_parts.push(format!(
                        "ALTER TABLE {} RENAME COLUMN {} TO {};",
                        qualified,
                        quote_ident(&old_column),
                        quote_ident(&new_column)
                    ));
                    Ok(ParseState::InChangeSet(cs))
                }
                "dropForeignKeyConstraint" => {
                    let Some(base_table) = get_attr(attrs, "baseTableName") else {
                        eprintln!(
                            "Warning: <dropForeignKeyConstraint> missing required 'baseTableName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let Some(constraint_name) = get_attr(attrs, "constraintName") else {
                        eprintln!(
                            "Warning: <dropForeignKeyConstraint> missing required 'constraintName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let schema_name = get_attr(attrs, "baseTableSchemaName");
                    let qualified = qualify_name(&schema_name, &base_table);
                    cs.sql_parts.push(format!(
                        "ALTER TABLE {} DROP CONSTRAINT {};",
                        qualified,
                        quote_ident(&constraint_name)
                    ));
                    Ok(ParseState::InChangeSet(cs))
                }
                "dropPrimaryKey" => {
                    let Some(table_name) = get_attr(attrs, "tableName") else {
                        eprintln!(
                            "Warning: <dropPrimaryKey> missing required 'tableName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let constraint_name = get_attr(attrs, "constraintName")
                        .unwrap_or_else(|| format!("{}_pkey", table_name));
                    let schema_name = get_attr(attrs, "schemaName");
                    let qualified = qualify_name(&schema_name, &table_name);
                    cs.sql_parts.push(format!(
                        "ALTER TABLE {} DROP CONSTRAINT {};",
                        qualified,
                        quote_ident(&constraint_name)
                    ));
                    Ok(ParseState::InChangeSet(cs))
                }
                "dropUniqueConstraint" => {
                    let Some(table_name) = get_attr(attrs, "tableName") else {
                        eprintln!(
                            "Warning: <dropUniqueConstraint> missing required 'tableName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let Some(constraint_name) = get_attr(attrs, "constraintName") else {
                        eprintln!(
                            "Warning: <dropUniqueConstraint> missing required 'constraintName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let schema_name = get_attr(attrs, "schemaName");
                    let qualified = qualify_name(&schema_name, &table_name);
                    cs.sql_parts.push(format!(
                        "ALTER TABLE {} DROP CONSTRAINT {};",
                        qualified,
                        quote_ident(&constraint_name)
                    ));
                    Ok(ParseState::InChangeSet(cs))
                }
                "renameTable" => {
                    let Some(old_table) = get_attr(attrs, "oldTableName") else {
                        eprintln!(
                            "Warning: <renameTable> missing required 'oldTableName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let Some(new_table) = get_attr(attrs, "newTableName") else {
                        eprintln!(
                            "Warning: <renameTable> missing required 'newTableName' attribute, skipping"
                        );
                        return Ok(ParseState::InChangeSet(cs));
                    };
                    let schema_name = get_attr(attrs, "schemaName");
                    let qualified = qualify_name(&schema_name, &old_table);
                    cs.sql_parts.push(format!(
                        "ALTER TABLE {} RENAME TO {};",
                        qualified,
                        quote_ident(&new_table)
                    ));
                    Ok(ParseState::InChangeSet(cs))
                }
                "rollback" | "preConditions" | "comment" | "tagDatabase" | "validCheckSum"
                | "property" => {
                    // Known non-SQL elements; skip silently
                    Ok(ParseState::InChangeSet(cs))
                }
                _ => {
                    // Unknown change type; emit a comment so catalog can detect incompleteness
                    if !is_empty {
                        // For non-empty unknown tags, we still need to stay in changeSet
                        // and the end tag will bring us back
                    }
                    cs.sql_parts.push(format!(
                        "-- pg-migration-lint: unsupported Liquibase change type '{}'",
                        tag_name
                    ));
                    Ok(ParseState::InChangeSet(cs))
                }
            }
        }
        ParseState::InSqlTag(cs, text) => {
            // Nested tags inside <sql> are unexpected; just stay collecting text
            Ok(ParseState::InSqlTag(cs, text))
        }
        ParseState::InCreateTable(cs, mut ct) => {
            if tag_name == "column" {
                match parse_column_element(attrs, is_empty) {
                    ParsedColumn::Complete(def) => {
                        ct.columns.push(def);
                        Ok(ParseState::InCreateTable(cs, ct))
                    }
                    ParsedColumn::Pending(col) => Ok(ParseState::InCreateTableColumn(cs, ct, col)),
                }
            } else {
                // Unknown child of createTable, ignore
                Ok(ParseState::InCreateTable(cs, ct))
            }
        }
        ParseState::InCreateTableColumn(cs, ct, mut col) => {
            if tag_name == "constraints" {
                col.constraints = parse_column_constraints(attrs);
            }
            Ok(ParseState::InCreateTableColumn(cs, ct, col))
        }
        ParseState::InAddColumn(cs, mut ac) => {
            if tag_name == "column" {
                match parse_column_element(attrs, is_empty) {
                    ParsedColumn::Complete(def) => {
                        ac.columns.push(def);
                        Ok(ParseState::InAddColumn(cs, ac))
                    }
                    ParsedColumn::Pending(col) => Ok(ParseState::InAddColumnColumn(cs, ac, col)),
                }
            } else {
                Ok(ParseState::InAddColumn(cs, ac))
            }
        }
        ParseState::InAddColumnColumn(cs, ac, mut col) => {
            if tag_name == "constraints" {
                col.constraints = parse_column_constraints(attrs);
            }
            Ok(ParseState::InAddColumnColumn(cs, ac, col))
        }
        ParseState::InCreateIndex(cs, mut ci) => {
            if tag_name == "column" {
                let name = get_attr(attrs, "name").unwrap_or_default();
                if !name.is_empty() {
                    ci.columns.push(name);
                }
            }
            Ok(ParseState::InCreateIndex(cs, ci))
        }
        ParseState::InDropColumn(cs, mut dc) => {
            if tag_name == "column"
                && let Some(name) = get_attr(attrs, "name")
            {
                dc.columns.push(name);
            }
            Ok(ParseState::InDropColumn(cs, dc))
        }
    }
}

/// Handle a closing XML tag, transitioning the state machine.
///
/// When a changeSet closes, its accumulated SQL parts are joined and
/// emitted as a `RawMigrationUnit`.
fn handle_end_tag(
    state: ParseState,
    tag_name: &str,
    source_path: &Path,
    units: &mut Vec<RawMigrationUnit>,
) -> Result<ParseState, LoadError> {
    match state {
        ParseState::Root => Ok(ParseState::Root),
        ParseState::InChangeSet(cs) => {
            if tag_name == "changeSet" {
                let sql = cs.sql_parts.join("\n");
                if !sql.is_empty() {
                    units.push(RawMigrationUnit {
                        id: cs.id,
                        sql,
                        source_file: source_path.to_path_buf(),
                        source_line_offset: cs.line,
                        run_in_transaction: cs.run_in_transaction,
                        is_down: false,
                    });
                }
                Ok(ParseState::Root)
            } else {
                // Closing a nested element we're not specifically tracking
                Ok(ParseState::InChangeSet(cs))
            }
        }
        ParseState::InSqlTag(mut cs, text) => {
            if tag_name == "sql" {
                let trimmed = text.trim().to_string();
                if !trimmed.is_empty() {
                    cs.sql_parts.push(trimmed);
                }
                Ok(ParseState::InChangeSet(cs))
            } else {
                Ok(ParseState::InSqlTag(cs, text))
            }
        }
        ParseState::InCreateTable(mut cs, ct) => {
            if tag_name == "createTable" {
                let stmts = generate_create_table_sql(&ct);
                cs.sql_parts.extend(stmts);
                Ok(ParseState::InChangeSet(cs))
            } else {
                Ok(ParseState::InCreateTable(cs, ct))
            }
        }
        ParseState::InCreateTableColumn(cs, mut ct, col) => {
            if tag_name == "column" {
                ct.columns.push(column_def_from_state(col));
                Ok(ParseState::InCreateTable(cs, ct))
            } else {
                Ok(ParseState::InCreateTableColumn(cs, ct, col))
            }
        }
        ParseState::InAddColumn(mut cs, ac) => {
            if tag_name == "addColumn" {
                for col_def in &ac.columns {
                    let sql = generate_add_column_sql(&ac.table_name, &ac.schema_name, col_def);
                    cs.sql_parts.push(sql);
                    if let Some(fk_sql) =
                        generate_inline_fk_sql(&ac.table_name, &ac.schema_name, col_def)
                    {
                        cs.sql_parts.push(fk_sql);
                    }
                }
                Ok(ParseState::InChangeSet(cs))
            } else {
                Ok(ParseState::InAddColumn(cs, ac))
            }
        }
        ParseState::InAddColumnColumn(cs, mut ac, col) => {
            if tag_name == "column" {
                ac.columns.push(column_def_from_state(col));
                Ok(ParseState::InAddColumn(cs, ac))
            } else {
                Ok(ParseState::InAddColumnColumn(cs, ac, col))
            }
        }
        ParseState::InCreateIndex(mut cs, ci) => {
            if tag_name == "createIndex" {
                let sql = generate_create_index_sql(&ci);
                cs.sql_parts.push(sql);
                Ok(ParseState::InChangeSet(cs))
            } else {
                Ok(ParseState::InCreateIndex(cs, ci))
            }
        }
        ParseState::InDropColumn(mut cs, dc) => {
            if tag_name == "dropColumn" {
                if dc.columns.is_empty() {
                    eprintln!(
                        "Warning: <dropColumn> for table '{}' has no column children, skipping",
                        dc.table_name
                    );
                } else {
                    let qualified = qualify_name(&dc.schema_name, &dc.table_name);
                    for col in &dc.columns {
                        cs.sql_parts.push(format!(
                            "ALTER TABLE {} DROP COLUMN {};",
                            qualified,
                            quote_ident(col)
                        ));
                    }
                }
                Ok(ParseState::InChangeSet(cs))
            } else {
                Ok(ParseState::InDropColumn(cs, dc))
            }
        }
    }
}

/// Handle text content within XML elements.
fn handle_text(state: ParseState, text: &str) -> ParseState {
    match state {
        ParseState::InSqlTag(cs, mut existing) => {
            if !existing.is_empty() {
                existing.push('\n');
            }
            existing.push_str(text);
            ParseState::InSqlTag(cs, existing)
        }
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Liquibase type mapping
// ---------------------------------------------------------------------------

/// Map Liquibase abstract type names to PostgreSQL types.
///
/// Liquibase uses abstract/Java-flavoured type names that don't correspond 1:1
/// to PostgreSQL types. The bridge JAR performs this mapping via the real
/// Liquibase engine; we need to replicate the important cases here so that the
/// XML fallback produces SQL that `pg_query` can parse and that rules like
/// PGM101 (timestamp without time zone) can detect.
fn map_liquibase_type(raw: &str) -> String {
    // Normalise to lowercase for matching, but preserve modifiers like (10,2)
    let lower = raw.to_ascii_lowercase();
    let (base, modifiers) = match lower.find('(') {
        Some(pos) => (&lower[..pos], &raw[pos..]),
        None => (lower.as_str(), ""),
    };

    let mapped = match base.trim() {
        "datetime" => "timestamp without time zone",
        "int" => "integer",
        "number" | "decimal" | "currency" => "numeric",
        "clob" | "nclob" => "text",
        "blob" => "bytea",
        "tinyint" => "smallint",
        "mediumint" => "integer",
        "double" => "double precision",
        "nvarchar" => "varchar",
        "nchar" => "char",
        "java.sql.types.timestamp" => "timestamp without time zone",
        "java.sql.types.date" => "date",
        "java.sql.types.time" => "time",
        "java.sql.types.varchar" => "varchar",
        _ => return raw.to_string(),
    };

    format!("{mapped}{modifiers}")
}

// ---------------------------------------------------------------------------
// Constraint / column helpers
// ---------------------------------------------------------------------------

/// Parse a `<constraints>` element's attributes into a `ColumnConstraints`.
fn parse_column_constraints(attrs: &[(String, String)]) -> ColumnConstraints {
    let nullable = get_attr(attrs, "nullable")
        .map(|v| v != "false")
        .unwrap_or(true);
    let primary_key = get_attr(attrs, "primaryKey")
        .map(|v| v == "true")
        .unwrap_or(false);
    let unique = get_attr(attrs, "unique")
        .map(|v| v == "true")
        .unwrap_or(false);
    let foreign_key_name = get_attr(attrs, "foreignKeyName");
    let mut referenced_table = get_attr(attrs, "referencedTableName");
    let mut referenced_schema = get_attr(attrs, "referencedTableSchemaName");
    let mut referenced_columns = get_attr(attrs, "referencedColumnNames");

    // Fall back to the `references` shorthand attribute, e.g. references="table_name (col1, col2)"
    if referenced_table.is_none()
        && let Some(refs) = get_attr(attrs, "references")
        && let Some((table, cols)) = parse_references_attr(&refs)
    {
        // Handle schema-qualified table: "myschema.table" → schema + table
        if let Some(dot) = table.find('.') {
            referenced_schema = Some(table[..dot].to_string());
            referenced_table = Some(table[dot + 1..].to_string());
        } else {
            referenced_table = Some(table);
        }
        referenced_columns = Some(cols);
    }

    ColumnConstraints {
        nullable,
        primary_key,
        unique,
        foreign_key_name,
        referenced_table,
        referenced_schema,
        referenced_columns,
    }
}

/// Parse the Liquibase `references` shorthand attribute.
///
/// Format: `"table_name (col1, col2)"` → `Some(("table_name", "col1, col2"))`
///
/// Schema-qualified references like `"myschema.table (col)"` are returned with
/// the dot-separated prefix in the table component. The caller splits on `.`
/// to populate `referenced_schema` and `referenced_table` separately.
fn parse_references_attr(refs: &str) -> Option<(String, String)> {
    let refs = refs.trim();
    let paren_start = refs.find('(')?;
    let paren_end = refs.rfind(')')?;
    if paren_end <= paren_start {
        return None;
    }
    let table = refs[..paren_start].trim().to_string();
    let cols = refs[paren_start + 1..paren_end].trim().to_string();
    if table.is_empty() || cols.is_empty() {
        return None;
    }
    Some((table, cols))
}

/// Result of parsing a `<column>` element's attributes.
enum ParsedColumn {
    /// Self-closing `<column .../>` — fully resolved to a `ColumnDef`.
    Complete(ColumnDef),
    /// Open `<column>` — needs child elements (e.g. `<constraints>`) before closing.
    Pending(ColumnState),
}

/// Parse a `<column>` element's attributes into either a complete `ColumnDef`
/// (self-closing) or a pending `ColumnState` (has children like `<constraints>`).
fn parse_column_element(attrs: &[(String, String)], is_empty: bool) -> ParsedColumn {
    let name = get_attr(attrs, "name").unwrap_or_default();
    let type_name = map_liquibase_type(&get_attr(attrs, "type").unwrap_or_default());
    let default_value = get_attr(attrs, "defaultValue")
        .or_else(|| get_attr(attrs, "defaultValueNumeric"))
        .or_else(|| get_attr(attrs, "defaultValueBoolean"))
        .or_else(|| get_attr(attrs, "defaultValueComputed"));

    if is_empty {
        ParsedColumn::Complete(ColumnDef {
            name,
            type_name,
            nullable: true,
            primary_key: false,
            unique: false,
            default_value,
            foreign_key_name: None,
            referenced_table: None,
            referenced_schema: None,
            referenced_columns: None,
        })
    } else {
        ParsedColumn::Pending(ColumnState {
            name,
            type_name,
            constraints: ColumnConstraints::default(),
            default_value,
        })
    }
}

/// Convert a `ColumnState` (parsing temp) into a `ColumnDef` (for SQL gen).
fn column_def_from_state(col: ColumnState) -> ColumnDef {
    ColumnDef {
        name: col.name,
        type_name: col.type_name,
        nullable: col.constraints.nullable,
        primary_key: col.constraints.primary_key,
        unique: col.constraints.unique,
        default_value: col.default_value,
        foreign_key_name: col.constraints.foreign_key_name,
        referenced_table: col.constraints.referenced_table,
        referenced_schema: col.constraints.referenced_schema,
        referenced_columns: col.constraints.referenced_columns,
    }
}

/// Generate inline FK SQL for a column, if it has FK constraints.
/// Returns `None` if the column has no FK reference.
fn generate_inline_fk_sql(
    table_name: &str,
    schema_name: &Option<String>,
    col: &ColumnDef,
) -> Option<String> {
    let ref_table = col.referenced_table.as_deref()?;
    let Some(ref_columns) = col.referenced_columns.as_deref() else {
        eprintln!(
            "Warning: inline FK on column '{}' references table '{}' but is missing \
             referencedColumnNames, skipping FK",
            col.name, ref_table
        );
        return None;
    };

    let base_qualified = qualify_name(schema_name, table_name);
    let ref_qualified = qualify_name(&col.referenced_schema, ref_table);
    let quoted_base_col = quote_ident(&col.name);
    let quoted_ref_cols = quote_column_list(ref_columns);

    let sql = if let Some(ref fk_name) = col.foreign_key_name {
        format!(
            "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({});",
            base_qualified,
            quote_ident(fk_name),
            quoted_base_col,
            ref_qualified,
            quoted_ref_cols
        )
    } else {
        format!(
            "ALTER TABLE {} ADD FOREIGN KEY ({}) REFERENCES {} ({});",
            base_qualified, quoted_base_col, ref_qualified, quoted_ref_cols
        )
    };
    Some(sql)
}

// ---------------------------------------------------------------------------
// SQL generation helpers
// ---------------------------------------------------------------------------

/// Generate CREATE TABLE and any inline FK SQL statements from parsed XML state.
/// Returns one or more SQL statements.
fn generate_create_table_sql(ct: &CreateTableState) -> Vec<String> {
    let qualified = qualify_name(&ct.schema_name, &ct.table_name);
    let mut col_parts: Vec<String> = Vec::new();

    for col in &ct.columns {
        let mut col_sql = format!("{} {}", quote_ident(&col.name), col.type_name);

        if col.primary_key {
            col_sql.push_str(" PRIMARY KEY");
        }

        if !col.nullable {
            col_sql.push_str(" NOT NULL");
        }

        if col.unique && !col.primary_key {
            col_sql.push_str(" UNIQUE");
        }

        if let Some(ref default_val) = col.default_value {
            col_sql.push_str(" DEFAULT ");
            col_sql.push_str(default_val);
        }

        col_parts.push(col_sql);
    }

    let mut stmts = vec![format!(
        "CREATE TABLE {} ({});",
        qualified,
        col_parts.join(", ")
    )];

    // Emit ALTER TABLE ADD CONSTRAINT for any inline FK constraints
    for col in &ct.columns {
        if let Some(fk_sql) = generate_inline_fk_sql(&ct.table_name, &ct.schema_name, col) {
            stmts.push(fk_sql);
        }
    }

    stmts
}

/// Generate an ALTER TABLE ADD COLUMN SQL statement.
fn generate_add_column_sql(
    table_name: &str,
    schema_name: &Option<String>,
    col: &ColumnDef,
) -> String {
    let qualified = qualify_name(schema_name, table_name);
    let mut sql = format!(
        "ALTER TABLE {} ADD COLUMN {} {}",
        qualified,
        quote_ident(&col.name),
        col.type_name
    );

    if !col.nullable {
        sql.push_str(" NOT NULL");
    }

    if col.unique {
        sql.push_str(" UNIQUE");
    }

    if let Some(ref default_val) = col.default_value {
        sql.push_str(" DEFAULT ");
        sql.push_str(default_val);
    }

    sql.push(';');
    sql
}

/// Generate a CREATE INDEX SQL statement from parsed XML state.
fn generate_create_index_sql(ci: &CreateIndexState) -> String {
    let table_qualified = qualify_name(&ci.schema_name, &ci.table_name);
    let unique_str = if ci.unique { "UNIQUE " } else { "" };
    let columns = ci
        .columns
        .iter()
        .map(|c| quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ");

    format!(
        "CREATE {}INDEX {} ON {} ({});",
        unique_str,
        quote_ident(&ci.index_name),
        table_qualified,
        columns
    )
}

/// Generate an ALTER TABLE ADD CONSTRAINT ... FOREIGN KEY SQL statement.
///
/// Required fields (`base_table`, `base_columns`, `ref_table`, `ref_columns`)
/// are passed as pre-validated parameters. Optional fields (`constraintName`,
/// schema names) are extracted from `attrs`.
fn generate_add_fk_sql(
    attrs: &[(String, String)],
    base_table: &str,
    base_columns: &str,
    ref_table: &str,
    ref_columns: &str,
) -> String {
    let constraint_name = get_attr(attrs, "constraintName").unwrap_or_default();
    let base_schema = get_attr(attrs, "baseTableSchemaName");
    let ref_schema = get_attr(attrs, "referencedTableSchemaName");

    let base_qualified = qualify_name(&base_schema, base_table);
    let ref_qualified = qualify_name(&ref_schema, ref_table);

    let quoted_base_cols = quote_column_list(base_columns);
    let quoted_ref_cols = quote_column_list(ref_columns);

    if constraint_name.is_empty() {
        format!(
            "ALTER TABLE {} ADD FOREIGN KEY ({}) REFERENCES {} ({});",
            base_qualified, quoted_base_cols, ref_qualified, quoted_ref_cols
        )
    } else {
        format!(
            "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({});",
            base_qualified,
            quote_ident(&constraint_name),
            quoted_base_cols,
            ref_qualified,
            quoted_ref_cols
        )
    }
}

/// Generate an ALTER TABLE ADD CONSTRAINT ... PRIMARY KEY SQL statement.
///
/// Required fields (`table_name`, `column_names`) are passed as pre-validated
/// parameters. Optional fields (`constraintName`, `schemaName`) are extracted
/// from `attrs`.
fn generate_add_pk_sql(attrs: &[(String, String)], table_name: &str, column_names: &str) -> String {
    let constraint_name = get_attr(attrs, "constraintName").unwrap_or_default();
    let schema_name = get_attr(attrs, "schemaName");

    let qualified = qualify_name(&schema_name, table_name);
    let quoted_cols = quote_column_list(column_names);

    if constraint_name.is_empty() {
        format!(
            "ALTER TABLE {} ADD PRIMARY KEY ({});",
            qualified, quoted_cols
        )
    } else {
        format!(
            "ALTER TABLE {} ADD CONSTRAINT {} PRIMARY KEY ({});",
            qualified,
            quote_ident(&constraint_name),
            quoted_cols
        )
    }
}

/// Generate an ALTER TABLE ADD CONSTRAINT ... UNIQUE SQL statement.
///
/// Required fields (`table_name`, `column_names`) are passed as pre-validated
/// parameters. Optional fields (`constraintName`, `schemaName`) are extracted
/// from `attrs`.
fn generate_add_unique_sql(
    attrs: &[(String, String)],
    table_name: &str,
    column_names: &str,
) -> String {
    let constraint_name = get_attr(attrs, "constraintName").unwrap_or_default();
    let schema_name = get_attr(attrs, "schemaName");

    let qualified = qualify_name(&schema_name, table_name);
    let quoted_cols = quote_column_list(column_names);

    if constraint_name.is_empty() {
        format!("ALTER TABLE {} ADD UNIQUE ({});", qualified, quoted_cols)
    } else {
        format!(
            "ALTER TABLE {} ADD CONSTRAINT {} UNIQUE ({});",
            qualified,
            quote_ident(&constraint_name),
            quoted_cols
        )
    }
}

// ---------------------------------------------------------------------------
// Utility helpers
// ---------------------------------------------------------------------------

/// Quote a SQL identifier (table, column, index, constraint name).
///
/// Always quotes to be safe — handles reserved words and special characters.
/// Embedded double quotes are escaped by doubling them.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Quote each element of a comma-separated column name list.
///
/// Splits on comma, trims whitespace, quotes each name individually,
/// then rejoins with `, `.
fn quote_column_list(columns: &str) -> String {
    columns
        .split(',')
        .map(|c| quote_ident(c.trim()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Qualify a name with an optional schema prefix, quoting both parts.
///
/// Returns `"schema"."name"` if schema is present, or just `"name"` otherwise.
fn qualify_name(schema: &Option<String>, name: &str) -> String {
    match schema {
        Some(s) if !s.is_empty() => format!("{}.{}", quote_ident(s), quote_ident(name)),
        _ => quote_ident(name),
    }
}

/// Get an attribute value by name from a collected attribute list.
fn get_attr(attrs: &[(String, String)], name: &str) -> Option<String> {
    attrs
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
}

/// Collect attributes from a quick-xml BytesStart into a Vec of (name, value) pairs.
fn collect_attributes(
    e: &quick_xml::events::BytesStart<'_>,
) -> Result<Vec<(String, String)>, LoadError> {
    let mut attrs = Vec::new();
    for attr_result in e.attributes() {
        let attr = attr_result.map_err(|err| LoadError::Parse {
            path: PathBuf::from("<xml>"),
            message: format!("Failed to parse XML attribute: {}", err),
        })?;
        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
        let value = String::from_utf8_lossy(&attr.value).to_string();
        attrs.push((key, value));
    }
    Ok(attrs)
}

/// Get the local name from a potentially namespace-prefixed tag name.
///
/// For example, `dbchangelog:changeSet` becomes `changeSet`.
fn local_name_str(name: &[u8]) -> String {
    let full = String::from_utf8_lossy(name);
    match full.rsplit_once(':') {
        Some((_, local)) => local.to_string(),
        None => full.to_string(),
    }
}

/// Convert a byte offset in the XML string to a 1-based line number.
fn byte_offset_to_line(xml: &str, offset: usize) -> usize {
    let clamped = offset.min(xml.len());
    xml[..clamped].matches('\n').count() + 1
}

/// Extract `<include file="..."/>` paths from the XML, resolved relative
/// to `base_dir`. Liquibase resolves include paths relative to the classpath
/// root (the directory of the top-level changelog), not the including file.
/// `source_file` is used only for error reporting.
fn extract_include_paths(
    xml: &str,
    base_dir: &Path,
    source_file: &Path,
) -> Result<Vec<PathBuf>, LoadError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text_start = true;
    reader.config_mut().trim_text_end = true;
    let mut buf = Vec::new();
    let mut paths = Vec::new();

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| LoadError::Parse {
                path: source_file.to_path_buf(),
                message: format!("XML parse error while scanning includes: {}", e),
            })?;

        match event {
            Event::Eof => break,
            Event::Start(ref e) | Event::Empty(ref e) => {
                let tag_name = local_name_str(e.name().as_ref());
                if tag_name == "include" {
                    let attrs = collect_attributes(e)?;
                    if let Some(file_attr) = get_attr(&attrs, "file") {
                        // Liquibase treats leading "/" as classpath-relative,
                        // not filesystem-absolute. Strip it so Path::join
                        // resolves relative to base_dir.
                        let cleaned = file_attr.strip_prefix('/').unwrap_or(&file_attr);

                        let relative_to_file = get_attr(&attrs, "relativeToChangelogFile")
                            .is_some_and(|v| v == "true");
                        let resolve_base = if relative_to_file {
                            source_file.parent().unwrap_or(base_dir)
                        } else {
                            base_dir
                        };

                        let resolved = resolve_base.join(cleaned);
                        paths.push(resolved);
                    }
                } else if tag_name == "includeAll" {
                    let attrs = collect_attributes(e)?;
                    if let Some(dir_attr) = get_attr(&attrs, "path") {
                        let cleaned = dir_attr.strip_prefix('/').unwrap_or(&dir_attr);

                        let relative_to_file = get_attr(&attrs, "relativeToChangelogFile")
                            .is_some_and(|v| v == "true");
                        let resolve_base = if relative_to_file {
                            source_file.parent().unwrap_or(base_dir)
                        } else {
                            base_dir
                        };
                        let resolved = resolve_base.join(cleaned);

                        if resolved.is_dir() {
                            let dir_entries: Vec<PathBuf> = std::fs::read_dir(&resolved)
                                .map_err(|e| LoadError::Io {
                                    path: resolved.clone(),
                                    source: e,
                                })?
                                .filter_map(|entry| entry.ok())
                                .map(|entry| entry.path())
                                .collect();

                            let has_subdirs = dir_entries.iter().any(|p| p.is_dir());
                            if has_subdirs {
                                eprintln!(
                                    "Warning: <includeAll> directory '{}' contains subdirectories which will not be scanned (the XML fallback parser does not support recursive includeAll — use the bridge JAR or liquibase update-sql strategy for nested layouts)",
                                    resolved.display()
                                );
                            }

                            let mut entries: Vec<PathBuf> = dir_entries
                                .into_iter()
                                .filter(|p| {
                                    p.is_file()
                                        && p.extension().is_some_and(|ext| {
                                            ext.eq_ignore_ascii_case("xml")
                                                || ext.eq_ignore_ascii_case("sql")
                                        })
                                })
                                .collect();
                            entries.sort_by(|a, b| {
                                a.file_name()
                                    .unwrap_or_default()
                                    .cmp(b.file_name().unwrap_or_default())
                            });
                            paths.extend(entries);
                        } else {
                            eprintln!(
                                "Warning: <includeAll> directory not found: {} (resolved from '{}')",
                                resolved.display(),
                                dir_attr
                            );
                        }
                    }
                }
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(paths)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_sql_changeset() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <sql>CREATE INDEX idx_users_name ON users (name);</sql>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse simple SQL changeset");
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].id, "1");
        assert_eq!(units[0].sql, "CREATE INDEX idx_users_name ON users (name);");
        assert!(units[0].run_in_transaction);
        assert!(!units[0].is_down);
    }

    #[test]
    fn test_create_table_with_columns_and_constraints() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <createTable tableName="users">
            <column name="id" type="integer">
                <constraints primaryKey="true" nullable="false"/>
            </column>
            <column name="name" type="varchar(100)">
                <constraints nullable="false"/>
            </column>
            <column name="email" type="varchar(255)"/>
        </createTable>
    </changeSet>
</databaseChangeLog>"#;

        let units =
            parse_changelog_xml(xml, Path::new("test.xml")).expect("Should parse createTable");
        assert_eq!(units.len(), 1);
        let sql = &units[0].sql;
        assert!(
            sql.contains("CREATE TABLE \"users\""),
            "Expected CREATE TABLE, got: {}",
            sql
        );
        assert!(
            sql.contains("\"id\" integer PRIMARY KEY NOT NULL"),
            "Expected PK column, got: {}",
            sql
        );
        assert!(
            sql.contains("\"name\" varchar(100) NOT NULL"),
            "Expected NOT NULL column, got: {}",
            sql
        );
        assert!(
            sql.contains("\"email\" varchar(255)"),
            "Expected nullable column, got: {}",
            sql
        );
    }

    #[test]
    fn test_add_column_changeset() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="2" author="dev">
        <addColumn tableName="users">
            <column name="age" type="integer">
                <constraints nullable="false"/>
            </column>
        </addColumn>
    </changeSet>
</databaseChangeLog>"#;

        let units =
            parse_changelog_xml(xml, Path::new("test.xml")).expect("Should parse addColumn");
        assert_eq!(units.len(), 1);
        let sql = &units[0].sql;
        assert!(
            sql.contains(r#"ALTER TABLE "users" ADD COLUMN "age" integer NOT NULL;"#),
            "Expected ADD COLUMN, got: {}",
            sql
        );
    }

    #[test]
    fn test_create_index_changeset() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="3" author="dev">
        <createIndex indexName="idx_users_email" tableName="users" unique="true">
            <column name="email"/>
        </createIndex>
    </changeSet>
</databaseChangeLog>"#;

        let units =
            parse_changelog_xml(xml, Path::new("test.xml")).expect("Should parse createIndex");
        assert_eq!(units.len(), 1);
        let sql = &units[0].sql;
        assert!(
            sql.contains(r#"CREATE UNIQUE INDEX "idx_users_email" ON "users" ("email");"#),
            "Expected CREATE UNIQUE INDEX, got: {}",
            sql
        );
    }

    #[test]
    fn test_empty_changelog() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
</databaseChangeLog>"#;

        let units =
            parse_changelog_xml(xml, Path::new("test.xml")).expect("Should parse empty changelog");
        assert!(units.is_empty());
    }

    #[test]
    fn test_multiple_changesets() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <createTable tableName="users">
            <column name="id" type="integer">
                <constraints primaryKey="true" nullable="false"/>
            </column>
            <column name="name" type="varchar(100)">
                <constraints nullable="false"/>
            </column>
        </createTable>
    </changeSet>
    <changeSet id="2" author="dev">
        <sql>CREATE INDEX idx_users_name ON users (name);</sql>
    </changeSet>
    <changeSet id="3" author="dev">
        <dropTable tableName="old_users"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse multiple changesets");
        assert_eq!(units.len(), 3);

        assert_eq!(units[0].id, "1");
        assert!(units[0].sql.contains("CREATE TABLE \"users\""));

        assert_eq!(units[1].id, "2");
        assert!(units[1].sql.contains("CREATE INDEX idx_users_name"));

        assert_eq!(units[2].id, "3");
        assert_eq!(units[2].sql, r#"DROP TABLE "old_users";"#);
    }

    #[test]
    fn test_run_in_transaction_false() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev" runInTransaction="false">
        <sql>CREATE INDEX CONCURRENTLY idx_big ON big_table (col);</sql>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse runInTransaction=false");
        assert_eq!(units.len(), 1);
        assert!(!units[0].run_in_transaction);
    }

    #[test]
    fn test_drop_table_changeset() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <dropTable tableName="old_table"/>
    </changeSet>
</databaseChangeLog>"#;

        let units =
            parse_changelog_xml(xml, Path::new("test.xml")).expect("Should parse dropTable");
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].sql, r#"DROP TABLE "old_table";"#);
    }

    #[test]
    fn test_drop_index_changeset() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <dropIndex indexName="idx_old"/>
    </changeSet>
</databaseChangeLog>"#;

        let units =
            parse_changelog_xml(xml, Path::new("test.xml")).expect("Should parse dropIndex");
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].sql, r#"DROP INDEX "idx_old";"#);
    }

    #[test]
    fn test_add_foreign_key_constraint() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <addForeignKeyConstraint
            constraintName="fk_order_user"
            baseTableName="orders"
            baseColumnNames="user_id"
            referencedTableName="users"
            referencedColumnNames="id"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse addForeignKeyConstraint");
        assert_eq!(units.len(), 1);
        let sql = &units[0].sql;
        assert!(
            sql.contains(r#"ALTER TABLE "orders" ADD CONSTRAINT "fk_order_user" FOREIGN KEY ("user_id") REFERENCES "users" ("id");"#),
            "Expected FK SQL, got: {}",
            sql
        );
    }

    #[test]
    fn test_add_primary_key() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <addPrimaryKey tableName="orders" columnNames="id" constraintName="pk_orders"/>
    </changeSet>
</databaseChangeLog>"#;

        let units =
            parse_changelog_xml(xml, Path::new("test.xml")).expect("Should parse addPrimaryKey");
        assert_eq!(units.len(), 1);
        let sql = &units[0].sql;
        assert!(
            sql.contains(r#"ALTER TABLE "orders" ADD CONSTRAINT "pk_orders" PRIMARY KEY ("id");"#),
            "Expected PK SQL, got: {}",
            sql
        );
    }

    #[test]
    fn test_add_unique_constraint() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <addUniqueConstraint tableName="users" columnNames="email" constraintName="uq_users_email"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse addUniqueConstraint");
        assert_eq!(units.len(), 1);
        let sql = &units[0].sql;
        assert!(
            sql.contains(
                r#"ALTER TABLE "users" ADD CONSTRAINT "uq_users_email" UNIQUE ("email");"#
            ),
            "Expected UNIQUE SQL, got: {}",
            sql
        );
    }

    #[test]
    fn test_schema_qualified_names() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <createTable tableName="users" schemaName="myschema">
            <column name="id" type="integer">
                <constraints primaryKey="true" nullable="false"/>
            </column>
        </createTable>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse schema-qualified createTable");
        assert_eq!(units.len(), 1);
        assert!(
            units[0].sql.contains(r#"CREATE TABLE "myschema"."users""#),
            "Expected schema-qualified name, got: {}",
            units[0].sql
        );
    }

    #[test]
    fn test_unknown_change_type_generates_comment() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <loadData tableName="users" file="data.csv"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse unknown change type");
        assert_eq!(units.len(), 1);
        assert!(
            units[0].sql.contains("unsupported Liquibase change type"),
            "Expected unsupported comment, got: {}",
            units[0].sql
        );
    }

    #[test]
    fn test_changeset_with_multiple_changes() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <createTable tableName="users">
            <column name="id" type="integer">
                <constraints primaryKey="true" nullable="false"/>
            </column>
        </createTable>
        <createIndex indexName="idx_users_id" tableName="users">
            <column name="id"/>
        </createIndex>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse changeset with multiple changes");
        assert_eq!(units.len(), 1);
        assert!(units[0].sql.contains("CREATE TABLE \"users\""));
        assert!(
            units[0]
                .sql
                .contains(r#"CREATE INDEX "idx_users_id" ON "users" ("id");"#)
        );
    }

    #[test]
    fn test_create_table_self_closing_columns() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <createTable tableName="simple">
            <column name="id" type="integer"/>
            <column name="name" type="text"/>
        </createTable>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse self-closing columns");
        assert_eq!(units.len(), 1);
        assert!(
            units[0]
                .sql
                .contains(r#"CREATE TABLE "simple" ("id" integer, "name" text);"#),
            "Expected simple CREATE TABLE, got: {}",
            units[0].sql
        );
    }

    #[test]
    fn test_source_file_and_line_offset() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="first" author="dev">
        <sql>SELECT 1;</sql>
    </changeSet>
</databaseChangeLog>"#;

        let path = Path::new("db/changelog/main.xml");
        let units = parse_changelog_xml(xml, path).expect("Should track source info");
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].source_file, PathBuf::from("db/changelog/main.xml"));
        // Line offset should be > 1 since changeSet is not on line 1
        assert!(
            units[0].source_line_offset > 1,
            "Expected line > 1, got: {}",
            units[0].source_line_offset
        );
    }

    #[test]
    fn test_empty_changeset_not_emitted() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="empty" author="dev">
        <comment>This changeset has no SQL-generating changes</comment>
    </changeSet>
</databaseChangeLog>"#;

        let units =
            parse_changelog_xml(xml, Path::new("test.xml")).expect("Should handle empty changeset");
        assert!(
            units.is_empty(),
            "Expected no units for empty changeset, got: {:?}",
            units
        );
    }

    #[test]
    fn test_sql_with_cdata() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <sql><![CDATA[
            CREATE TABLE test (id int, data jsonb);
        ]]></sql>
    </changeSet>
</databaseChangeLog>"#;

        let units =
            parse_changelog_xml(xml, Path::new("test.xml")).expect("Should parse CDATA SQL");
        assert_eq!(units.len(), 1);
        assert!(
            units[0].sql.contains("CREATE TABLE test"),
            "Expected SQL from CDATA, got: {}",
            units[0].sql
        );
    }

    #[test]
    fn test_multi_column_create_index() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <createIndex indexName="idx_orders_composite" tableName="orders">
            <column name="user_id"/>
            <column name="created_at"/>
        </createIndex>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse multi-column index");
        assert_eq!(units.len(), 1);
        assert!(
            units[0].sql.contains(
                r#"CREATE INDEX "idx_orders_composite" ON "orders" ("user_id", "created_at");"#
            ),
            "Expected multi-column index, got: {}",
            units[0].sql
        );
    }

    #[test]
    fn test_add_fk_with_schemas() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <addForeignKeyConstraint
            constraintName="fk_order_user"
            baseTableSchemaName="sales"
            baseTableName="orders"
            baseColumnNames="user_id"
            referencedTableSchemaName="accounts"
            referencedTableName="users"
            referencedColumnNames="id"/>
    </changeSet>
</databaseChangeLog>"#;

        let units =
            parse_changelog_xml(xml, Path::new("test.xml")).expect("Should parse FK with schemas");
        assert_eq!(units.len(), 1);
        let sql = &units[0].sql;
        assert!(
            sql.contains(r#"ALTER TABLE "sales"."orders" ADD CONSTRAINT "fk_order_user" FOREIGN KEY ("user_id") REFERENCES "accounts"."users" ("id");"#),
            "Expected schema-qualified FK, got: {}",
            sql
        );
    }

    #[test]
    fn test_add_primary_key_without_constraint_name() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <addPrimaryKey tableName="orders" columnNames="id"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse addPrimaryKey without name");
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].sql,
            r#"ALTER TABLE "orders" ADD PRIMARY KEY ("id");"#
        );
    }

    #[test]
    fn test_byte_offset_to_line() {
        let text = "line1\nline2\nline3\n";
        assert_eq!(byte_offset_to_line(text, 0), 1);
        assert_eq!(byte_offset_to_line(text, 5), 1); // at the \n
        assert_eq!(byte_offset_to_line(text, 6), 2); // start of line2
        assert_eq!(byte_offset_to_line(text, 12), 3); // start of line3
    }

    #[test]
    fn test_qualify_name() {
        assert_eq!(qualify_name(&None, "users"), "\"users\"");
        assert_eq!(
            qualify_name(&Some("public".to_string()), "users"),
            "\"public\".\"users\""
        );
        assert_eq!(qualify_name(&Some("".to_string()), "users"), "\"users\"");
    }

    #[test]
    fn test_quote_ident() {
        assert_eq!(quote_ident("users"), "\"users\"");
        assert_eq!(quote_ident("order"), "\"order\"");
        assert_eq!(quote_ident("has\"quote"), "\"has\"\"quote\"");
    }

    #[test]
    fn test_quote_column_list() {
        assert_eq!(quote_column_list("col1,col2"), "\"col1\", \"col2\"");
        assert_eq!(
            quote_column_list("col1, col2, col3"),
            "\"col1\", \"col2\", \"col3\""
        );
        assert_eq!(quote_column_list("single"), "\"single\"");
    }

    #[test]
    fn test_local_name_str() {
        assert_eq!(local_name_str(b"changeSet"), "changeSet");
        assert_eq!(local_name_str(b"dbchangelog:changeSet"), "changeSet");
    }

    #[test]
    fn test_extract_include_paths() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <include file="sub/changelog-1.xml"/>
    <include file="sub/changelog-2.xml"/>
</databaseChangeLog>"#;

        let paths = extract_include_paths(
            xml,
            Path::new("/project/db"),
            Path::new("/project/db/main.xml"),
        )
        .expect("Should extract include paths");
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], PathBuf::from("/project/db/sub/changelog-1.xml"));
        assert_eq!(paths[1], PathBuf::from("/project/db/sub/changelog-2.xml"));
    }

    #[test]
    fn test_extract_include_paths_strips_leading_slash() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <include file="/sub/changelog.xml"/>
</databaseChangeLog>"#;

        let paths = extract_include_paths(
            xml,
            Path::new("/project/db"),
            Path::new("/project/db/main.xml"),
        )
        .expect("Should strip leading slash");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], PathBuf::from("/project/db/sub/changelog.xml"));
    }

    #[test]
    fn test_include_relative_to_changelog_file() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <include file="001-create-tables.xml" relativeToChangelogFile="true"/>
</databaseChangeLog>"#;

        let paths = extract_include_paths(
            xml,
            Path::new("/project"),
            Path::new("/project/db/changelog/master.xml"),
        )
        .expect("Should resolve relative to changelog file");
        assert_eq!(paths.len(), 1);
        assert_eq!(
            paths[0],
            PathBuf::from("/project/db/changelog/001-create-tables.xml")
        );
    }

    #[test]
    fn test_include_without_relative_flag_uses_base_dir() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <include file="001-create-tables.xml"/>
</databaseChangeLog>"#;

        let paths = extract_include_paths(
            xml,
            Path::new("/project"),
            Path::new("/project/db/changelog/master.xml"),
        )
        .expect("Should resolve relative to base dir");
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], PathBuf::from("/project/001-create-tables.xml"));
    }

    #[test]
    fn test_include_all_discovers_files_sorted() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let changesets_dir = dir.path().join("changesets");
        std::fs::create_dir(&changesets_dir).expect("mkdir");

        // Create files in non-alphabetical order
        std::fs::write(
            changesets_dir.join("003-third.xml"),
            r#"<?xml version="1.0"?><databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog"/>"#,
        )
        .expect("write");
        std::fs::write(
            changesets_dir.join("001-first.xml"),
            r#"<?xml version="1.0"?><databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog"/>"#,
        )
        .expect("write");
        std::fs::write(changesets_dir.join("002-second.sql"), "SELECT 1;").expect("write");
        // Non-matching file should be ignored
        std::fs::write(changesets_dir.join("readme.txt"), "ignore me").expect("write");

        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <includeAll path="changesets/"/>
</databaseChangeLog>"#;

        let source_file = dir.path().join("master.xml");
        let paths = extract_include_paths(xml, dir.path(), &source_file)
            .expect("Should extract includeAll paths");
        assert_eq!(paths.len(), 3, "Expected 3 files, got: {:?}", paths);
        // Should be sorted by filename
        assert!(
            paths[0].ends_with("001-first.xml"),
            "First should be 001, got: {:?}",
            paths[0]
        );
        assert!(
            paths[1].ends_with("002-second.sql"),
            "Second should be 002, got: {:?}",
            paths[1]
        );
        assert!(
            paths[2].ends_with("003-third.xml"),
            "Third should be 003, got: {:?}",
            paths[2]
        );
    }

    #[test]
    fn test_include_all_missing_dir_does_not_error() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <includeAll path="nonexistent/"/>
</databaseChangeLog>"#;

        let paths = extract_include_paths(
            xml,
            Path::new("/tmp/fake"),
            Path::new("/tmp/fake/master.xml"),
        )
        .expect("Missing dir should not error");
        assert!(paths.is_empty());
    }

    #[test]
    fn test_include_all_relative_to_changelog_file() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let sub_dir = dir.path().join("sub");
        std::fs::create_dir(&sub_dir).expect("mkdir sub");
        let changesets_dir = sub_dir.join("changesets");
        std::fs::create_dir(&changesets_dir).expect("mkdir changesets");

        std::fs::write(
            changesets_dir.join("001.xml"),
            r#"<?xml version="1.0"?><databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog"/>"#,
        )
        .expect("write");

        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <includeAll path="changesets/" relativeToChangelogFile="true"/>
</databaseChangeLog>"#;

        let source_file = sub_dir.join("changelog.xml");
        let paths = extract_include_paths(xml, dir.path(), &source_file)
            .expect("Should resolve relative to changelog file");
        assert_eq!(paths.len(), 1);
        assert!(
            paths[0].ends_with("001.xml"),
            "Expected 001.xml, got: {:?}",
            paths[0]
        );
    }

    #[test]
    fn test_run_in_transaction_default_true() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <sql>SELECT 1;</sql>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should default to run_in_transaction=true");
        assert_eq!(units.len(), 1);
        assert!(units[0].run_in_transaction);
    }

    #[test]
    fn test_add_fk_without_constraint_name() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <addForeignKeyConstraint
            baseTableName="orders"
            baseColumnNames="user_id"
            referencedTableName="users"
            referencedColumnNames="id"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse addForeignKeyConstraint without constraintName");
        assert_eq!(units.len(), 1);
        let sql = &units[0].sql;
        assert_eq!(
            sql, r#"ALTER TABLE "orders" ADD FOREIGN KEY ("user_id") REFERENCES "users" ("id");"#,
            "Expected valid FK SQL without CONSTRAINT keyword, got: {}",
            sql
        );
    }

    #[test]
    fn test_drop_index_missing_index_name() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <dropIndex/>
        <sql>SELECT 1;</sql>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should handle dropIndex without indexName");
        assert_eq!(units.len(), 1);
        let sql = &units[0].sql;
        assert!(
            !sql.contains("DROP INDEX"),
            "Expected no DROP INDEX for missing indexName, got: {}",
            sql
        );
        assert!(
            sql.contains("SELECT 1;"),
            "Expected subsequent SQL to still be processed, got: {}",
            sql
        );
    }

    #[test]
    fn test_add_fk_missing_base_table_name() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <addForeignKeyConstraint
            constraintName="fk_test"
            baseColumnNames="user_id"
            referencedTableName="users"
            referencedColumnNames="id"/>
        <sql>SELECT 1;</sql>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should handle addForeignKeyConstraint without baseTableName");
        assert_eq!(units.len(), 1);
        let sql = &units[0].sql;
        assert!(
            !sql.contains("FOREIGN KEY"),
            "Expected no FK SQL for missing baseTableName, got: {}",
            sql
        );
        assert!(
            sql.contains("SELECT 1;"),
            "Expected subsequent SQL to still be processed, got: {}",
            sql
        );
    }

    #[test]
    fn test_add_primary_key_missing_column_names() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <addPrimaryKey tableName="orders"/>
        <sql>SELECT 1;</sql>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should handle addPrimaryKey without columnNames");
        assert_eq!(units.len(), 1);
        let sql = &units[0].sql;
        assert!(
            !sql.contains("PRIMARY KEY"),
            "Expected no PRIMARY KEY SQL for missing columnNames, got: {}",
            sql
        );
        assert!(
            sql.contains("SELECT 1;"),
            "Expected subsequent SQL to still be processed, got: {}",
            sql
        );
    }

    #[test]
    fn test_add_unique_constraint_missing_table_name() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <addUniqueConstraint columnNames="email" constraintName="uq_test"/>
        <sql>SELECT 1;</sql>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should handle addUniqueConstraint without tableName");
        assert_eq!(units.len(), 1);
        let sql = &units[0].sql;
        assert!(
            !sql.contains("UNIQUE"),
            "Expected no UNIQUE SQL for missing tableName, got: {}",
            sql
        );
        assert!(
            sql.contains("SELECT 1;"),
            "Expected subsequent SQL to still be processed, got: {}",
            sql
        );
    }

    #[test]
    fn test_duplicate_include_loads_changeset_only_once() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");

        // child.xml contains one changeset
        let child_path = dir.path().join("child.xml");
        std::fs::write(
            &child_path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="child-1" author="dev">
        <sql>CREATE TABLE widgets (id int);</sql>
    </changeSet>
</databaseChangeLog>"#,
        )
        .expect("Failed to write child.xml");

        // master.xml includes child.xml twice
        let master_path = dir.path().join("master.xml");
        std::fs::write(
            &master_path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <include file="child.xml"/>
    <include file="child.xml"/>
</databaseChangeLog>"#,
        )
        .expect("Failed to write master.xml");

        let loader = XmlFallbackLoader;
        let units = loader
            .load(&master_path)
            .expect("Should handle duplicate includes");

        // The changeset from child.xml must appear exactly once
        assert_eq!(
            units.len(),
            1,
            "Expected 1 unit (duplicate include should be skipped), got {}",
            units.len()
        );
        assert_eq!(units[0].id, "child-1");
        assert!(
            units[0].sql.contains("CREATE TABLE widgets"),
            "Expected child changeset SQL, got: {}",
            units[0].sql
        );
    }

    #[test]
    fn test_circular_include_does_not_loop() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");

        // file_a.xml includes file_b.xml and has its own changeset
        let file_a_path = dir.path().join("file_a.xml");
        let file_b_path = dir.path().join("file_b.xml");

        std::fs::write(
            &file_a_path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="a-1" author="dev">
        <sql>CREATE TABLE alpha (id int);</sql>
    </changeSet>
    <include file="file_b.xml"/>
</databaseChangeLog>"#,
        )
        .expect("Failed to write file_a.xml");

        // file_b.xml includes file_a.xml (circular) and has its own changeset
        std::fs::write(
            &file_b_path,
            r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="b-1" author="dev">
        <sql>CREATE TABLE beta (id int);</sql>
    </changeSet>
    <include file="file_a.xml"/>
</databaseChangeLog>"#,
        )
        .expect("Failed to write file_b.xml");

        let loader = XmlFallbackLoader;
        let units = loader
            .load(&file_a_path)
            .expect("Should handle circular includes without infinite loop");

        // Both changesets should appear exactly once
        assert_eq!(
            units.len(),
            2,
            "Expected 2 units (one from each file), got {}",
            units.len()
        );

        let ids: Vec<&str> = units.iter().map(|u| u.id.as_str()).collect();
        assert!(
            ids.contains(&"a-1"),
            "Expected changeset a-1, got: {:?}",
            ids
        );
        assert!(
            ids.contains(&"b-1"),
            "Expected changeset b-1, got: {:?}",
            ids
        );

        // Verify SQL content
        let a_unit = units.iter().find(|u| u.id == "a-1").unwrap();
        assert!(
            a_unit.sql.contains("CREATE TABLE alpha"),
            "Expected alpha SQL, got: {}",
            a_unit.sql
        );
        let b_unit = units.iter().find(|u| u.id == "b-1").unwrap();
        assert!(
            b_unit.sql.contains("CREATE TABLE beta"),
            "Expected beta SQL, got: {}",
            b_unit.sql
        );
    }

    #[test]
    fn test_parse_liquibase_formatted_sql() {
        let content = "\
--liquibase formatted sql

--changeset alice:create-users
CREATE TABLE users (id int PRIMARY KEY);

--changeset bob:add-email runInTransaction:false
ALTER TABLE users ADD COLUMN email text;

--changeset carol:seed-data
INSERT INTO users (id, email) VALUES (1, 'test@example.com');
";
        let path = Path::new("test.sql");
        let units =
            parse_liquibase_formatted_sql(content, path).expect("Should parse formatted SQL");

        assert_eq!(units.len(), 3, "Expected 3 changesets, got {}", units.len());

        // First changeset
        assert_eq!(units[0].id, "create-users");
        assert!(units[0].sql.contains("CREATE TABLE users"));
        assert!(units[0].run_in_transaction);

        // Second changeset — runInTransaction:false
        assert_eq!(units[1].id, "add-email");
        assert!(units[1].sql.contains("ALTER TABLE users ADD COLUMN email"));
        assert!(!units[1].run_in_transaction);

        // Third changeset
        assert_eq!(units[2].id, "seed-data");
        assert!(units[2].sql.contains("INSERT INTO users"));
        assert!(units[2].run_in_transaction);
    }

    #[test]
    fn test_parse_liquibase_formatted_sql_skips_directives() {
        let content = "\
--liquibase formatted sql

--changeset dev:create-table
CREATE TABLE things (id int);
--rollback DROP TABLE things;
--comment This adds the things table
";
        let path = Path::new("test.sql");
        let units =
            parse_liquibase_formatted_sql(content, path).expect("Should parse formatted SQL");

        assert_eq!(units.len(), 1);
        assert_eq!(units[0].id, "create-table");
        assert!(
            !units[0].sql.contains("rollback"),
            "Rollback directive should be skipped, got: {}",
            units[0].sql
        );
        assert!(
            !units[0].sql.contains("comment"),
            "Comment directive should be skipped, got: {}",
            units[0].sql
        );
    }

    #[test]
    fn test_parse_liquibase_formatted_sql_no_changesets() {
        let content = "\
--liquibase formatted sql
-- just a comment, no changesets
";
        let path = Path::new("test.sql");
        let units =
            parse_liquibase_formatted_sql(content, path).expect("Should parse formatted SQL");

        assert_eq!(
            units.len(),
            0,
            "Expected 0 changesets for file with no --changeset markers"
        );
    }

    #[test]
    fn test_malformed_xml_returns_no_units() {
        // Streaming XML parsers (quick-xml) don't necessarily error on truncated input;
        // they process available events and then hit EOF. Since the <changeSet> is never
        // closed, no RawMigrationUnit is emitted, so we expect Ok with an empty vec.
        let xml = r#"<?xml version="1.0"?>
<databaseChangeLog>
    <changeSet id="1" author="dev">
        <sql>SELECT 1;
    <!-- missing closing tags -->"#;

        let result = parse_changelog_xml(xml, Path::new("test.xml"));
        match result {
            Ok(units) => assert!(
                units.is_empty(),
                "Expected no units from truncated XML, got {} unit(s)",
                units.len()
            ),
            Err(_) => {
                // Also acceptable: some quick-xml versions may error on truncated input
            }
        }
    }

    // -----------------------------------------------------------------------
    // Fix 1: dropColumn
    // -----------------------------------------------------------------------

    #[test]
    fn test_drop_column_single() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <dropColumn tableName="users" columnName="email"/>
    </changeSet>
</databaseChangeLog>"#;

        let units =
            parse_changelog_xml(xml, Path::new("test.xml")).expect("Should parse dropColumn");
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].sql, r#"ALTER TABLE "users" DROP COLUMN "email";"#);
    }

    #[test]
    fn test_drop_column_multi() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <dropColumn tableName="users">
            <column name="email"/>
            <column name="phone"/>
        </dropColumn>
    </changeSet>
</databaseChangeLog>"#;

        let units =
            parse_changelog_xml(xml, Path::new("test.xml")).expect("Should parse multi dropColumn");
        assert_eq!(units.len(), 1);
        let sql = &units[0].sql;
        assert!(
            sql.contains(r#"ALTER TABLE "users" DROP COLUMN "email";"#),
            "Expected DROP COLUMN email, got: {}",
            sql
        );
        assert!(
            sql.contains(r#"ALTER TABLE "users" DROP COLUMN "phone";"#),
            "Expected DROP COLUMN phone, got: {}",
            sql
        );
    }

    #[test]
    fn test_drop_column_with_schema() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <dropColumn tableName="users" schemaName="myschema" columnName="email"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse schema-qualified dropColumn");
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].sql,
            r#"ALTER TABLE "myschema"."users" DROP COLUMN "email";"#
        );
    }

    // -----------------------------------------------------------------------
    // Fix 2: modifyDataType
    // -----------------------------------------------------------------------

    #[test]
    fn test_modify_data_type() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <modifyDataType tableName="users" columnName="name" newDataType="text"/>
    </changeSet>
</databaseChangeLog>"#;

        let units =
            parse_changelog_xml(xml, Path::new("test.xml")).expect("Should parse modifyDataType");
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].sql,
            r#"ALTER TABLE "users" ALTER COLUMN "name" TYPE text;"#
        );
    }

    #[test]
    fn test_modify_data_type_with_schema() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <modifyDataType tableName="users" schemaName="myschema" columnName="name" newDataType="varchar(500)"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse schema-qualified modifyDataType");
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].sql,
            r#"ALTER TABLE "myschema"."users" ALTER COLUMN "name" TYPE varchar(500);"#
        );
    }

    // -----------------------------------------------------------------------
    // Fix 5: addNotNullConstraint / dropNotNullConstraint
    // -----------------------------------------------------------------------

    #[test]
    fn test_add_not_null_constraint() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <addNotNullConstraint tableName="users" columnName="email"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse addNotNullConstraint");
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].sql,
            r#"ALTER TABLE "users" ALTER COLUMN "email" SET NOT NULL;"#
        );
    }

    #[test]
    fn test_drop_not_null_constraint() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <dropNotNullConstraint tableName="users" columnName="email"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse dropNotNullConstraint");
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].sql,
            r#"ALTER TABLE "users" ALTER COLUMN "email" DROP NOT NULL;"#
        );
    }

    // -----------------------------------------------------------------------
    // renameColumn
    // -----------------------------------------------------------------------

    #[test]
    fn test_rename_column() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <renameColumn tableName="users" oldColumnName="fname" newColumnName="first_name"/>
    </changeSet>
</databaseChangeLog>"#;

        let units =
            parse_changelog_xml(xml, Path::new("test.xml")).expect("Should parse renameColumn");
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].sql,
            r#"ALTER TABLE "users" RENAME COLUMN "fname" TO "first_name";"#
        );
    }

    #[test]
    fn test_rename_column_with_schema() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <renameColumn tableName="users" oldColumnName="fname" newColumnName="first_name" schemaName="myschema"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse renameColumn with schema");
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].sql,
            r#"ALTER TABLE "myschema"."users" RENAME COLUMN "fname" TO "first_name";"#
        );
    }

    #[test]
    fn test_rename_column_missing_old_column_name() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <renameColumn tableName="users" newColumnName="first_name"/>
        <sql>SELECT 1;</sql>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should handle renameColumn without oldColumnName");
        assert_eq!(units.len(), 1);
        assert!(
            !units[0].sql.contains("RENAME COLUMN"),
            "Expected no RENAME COLUMN for missing oldColumnName, got: {}",
            units[0].sql
        );
        assert!(units[0].sql.contains("SELECT 1;"));
    }

    // -----------------------------------------------------------------------
    // dropForeignKeyConstraint
    // -----------------------------------------------------------------------

    #[test]
    fn test_drop_foreign_key_constraint() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <dropForeignKeyConstraint baseTableName="orders" constraintName="fk_order_user"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse dropForeignKeyConstraint");
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].sql,
            r#"ALTER TABLE "orders" DROP CONSTRAINT "fk_order_user";"#
        );
    }

    #[test]
    fn test_drop_foreign_key_constraint_with_schema() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <dropForeignKeyConstraint baseTableName="orders" constraintName="fk_order_user" baseTableSchemaName="myschema"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse dropForeignKeyConstraint with schema");
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].sql,
            r#"ALTER TABLE "myschema"."orders" DROP CONSTRAINT "fk_order_user";"#
        );
    }

    #[test]
    fn test_drop_foreign_key_constraint_missing_constraint_name() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <dropForeignKeyConstraint baseTableName="orders"/>
        <sql>SELECT 1;</sql>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should handle dropForeignKeyConstraint without constraintName");
        assert_eq!(units.len(), 1);
        assert!(
            !units[0].sql.contains("DROP CONSTRAINT"),
            "Expected no DROP CONSTRAINT for missing constraintName, got: {}",
            units[0].sql
        );
        assert!(units[0].sql.contains("SELECT 1;"));
    }

    // -----------------------------------------------------------------------
    // dropPrimaryKey
    // -----------------------------------------------------------------------

    #[test]
    fn test_drop_primary_key() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <dropPrimaryKey tableName="orders" constraintName="pk_orders"/>
    </changeSet>
</databaseChangeLog>"#;

        let units =
            parse_changelog_xml(xml, Path::new("test.xml")).expect("Should parse dropPrimaryKey");
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].sql,
            r#"ALTER TABLE "orders" DROP CONSTRAINT "pk_orders";"#
        );
    }

    #[test]
    fn test_drop_primary_key_synthesized_name() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <dropPrimaryKey tableName="orders"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse dropPrimaryKey with synthesized name");
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].sql,
            r#"ALTER TABLE "orders" DROP CONSTRAINT "orders_pkey";"#
        );
    }

    #[test]
    fn test_drop_primary_key_with_schema() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <dropPrimaryKey tableName="orders" constraintName="pk_orders" schemaName="myschema"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse dropPrimaryKey with schema");
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].sql,
            r#"ALTER TABLE "myschema"."orders" DROP CONSTRAINT "pk_orders";"#
        );
    }

    #[test]
    fn test_drop_primary_key_missing_table_name() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <dropPrimaryKey constraintName="pk_orders"/>
        <sql>SELECT 1;</sql>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should handle dropPrimaryKey without tableName");
        assert_eq!(units.len(), 1);
        assert!(
            !units[0].sql.contains("DROP CONSTRAINT"),
            "Expected no DROP CONSTRAINT for missing tableName, got: {}",
            units[0].sql
        );
        assert!(units[0].sql.contains("SELECT 1;"));
    }

    // -----------------------------------------------------------------------
    // dropUniqueConstraint
    // -----------------------------------------------------------------------

    #[test]
    fn test_drop_unique_constraint() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <dropUniqueConstraint tableName="users" constraintName="uq_users_email"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse dropUniqueConstraint");
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].sql,
            r#"ALTER TABLE "users" DROP CONSTRAINT "uq_users_email";"#
        );
    }

    #[test]
    fn test_drop_unique_constraint_with_schema() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <dropUniqueConstraint tableName="users" constraintName="uq_users_email" schemaName="myschema"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse dropUniqueConstraint with schema");
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].sql,
            r#"ALTER TABLE "myschema"."users" DROP CONSTRAINT "uq_users_email";"#
        );
    }

    #[test]
    fn test_drop_unique_constraint_missing_constraint_name() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <dropUniqueConstraint tableName="users"/>
        <sql>SELECT 1;</sql>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should handle dropUniqueConstraint without constraintName");
        assert_eq!(units.len(), 1);
        assert!(
            !units[0].sql.contains("DROP CONSTRAINT"),
            "Expected no DROP CONSTRAINT for missing constraintName, got: {}",
            units[0].sql
        );
        assert!(units[0].sql.contains("SELECT 1;"));
    }

    // -----------------------------------------------------------------------
    // renameTable
    // -----------------------------------------------------------------------

    #[test]
    fn test_rename_table() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <renameTable oldTableName="users" newTableName="app_users"/>
    </changeSet>
</databaseChangeLog>"#;

        let units =
            parse_changelog_xml(xml, Path::new("test.xml")).expect("Should parse renameTable");
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].sql,
            r#"ALTER TABLE "users" RENAME TO "app_users";"#
        );
    }

    #[test]
    fn test_rename_table_with_schema() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <renameTable oldTableName="users" newTableName="app_users" schemaName="myschema"/>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse renameTable with schema");
        assert_eq!(units.len(), 1);
        assert_eq!(
            units[0].sql,
            r#"ALTER TABLE "myschema"."users" RENAME TO "app_users";"#
        );
    }

    #[test]
    fn test_rename_table_missing_new_table_name() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <renameTable oldTableName="users"/>
        <sql>SELECT 1;</sql>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should handle renameTable without newTableName");
        assert_eq!(units.len(), 1);
        assert!(
            !units[0].sql.contains("RENAME TO"),
            "Expected no RENAME TO for missing newTableName, got: {}",
            units[0].sql
        );
        assert!(units[0].sql.contains("SELECT 1;"));
    }

    #[test]
    fn test_create_table_with_inline_foreign_key() {
        // Based on real migration: add_proposal + add_proposal_validation_error
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="add_proposal" author="jeff">
        <createTable tableName="proposal">
            <column name="proposal_id" type="uuid">
                <constraints nullable="false" primaryKey="true"/>
            </column>
            <column name="status" type="varchar(30)">
                <constraints nullable="false"/>
            </column>
        </createTable>
    </changeSet>
    <changeSet id="add_proposal_validation_error" author="victor">
        <createTable tableName="proposal_validation_error">
            <column name="proposal_id" type="uuid">
                <constraints nullable="false" referencedTableName="proposal" referencedColumnNames="proposal_id"
                             foreignKeyName="fk_proposal"/>
            </column>
            <column name="error_type" type="varchar(30)">
                <constraints nullable="false"/>
            </column>
        </createTable>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse createTable with inline FK");
        assert_eq!(units.len(), 2);

        // First changeset: just a CREATE TABLE, no FK
        assert!(
            !units[0].sql.contains("FOREIGN KEY"),
            "proposal should have no FK, got: {}",
            units[0].sql
        );

        // Second changeset: CREATE TABLE + ALTER TABLE ADD CONSTRAINT FK
        let sql = &units[1].sql;
        assert!(
            sql.contains("CREATE TABLE \"proposal_validation_error\""),
            "Expected CREATE TABLE, got: {}",
            sql
        );
        assert!(
            sql.contains("FOREIGN KEY"),
            "Expected FOREIGN KEY statement, got: {}",
            sql
        );
        assert!(
            sql.contains("REFERENCES \"proposal\""),
            "Expected REFERENCES proposal, got: {}",
            sql
        );
        assert!(
            sql.contains("\"fk_proposal\""),
            "Expected constraint name fk_proposal, got: {}",
            sql
        );
    }

    #[test]
    fn test_add_column_with_inline_foreign_key() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <addColumn tableName="invoice">
            <column name="client_id" type="uuid">
                <constraints nullable="false"
                             foreignKeyName="fk_invoice_client"
                             referencedTableName="client"
                             referencedColumnNames="client_id"/>
            </column>
        </addColumn>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse addColumn with inline FK");
        assert_eq!(units.len(), 1);
        let sql = &units[0].sql;
        assert!(
            sql.contains("ADD COLUMN"),
            "Expected ADD COLUMN, got: {}",
            sql
        );
        assert!(
            sql.contains("FOREIGN KEY (\"client_id\") REFERENCES \"client\""),
            "Expected FK referencing client, got: {}",
            sql
        );
        assert!(
            sql.contains("\"fk_invoice_client\""),
            "Expected constraint name, got: {}",
            sql
        );
    }

    #[test]
    fn test_inline_fk_without_constraint_name() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <createTable tableName="order_item">
            <column name="order_id" type="uuid">
                <constraints nullable="false"
                             referencedTableName="orders"
                             referencedColumnNames="order_id"/>
            </column>
        </createTable>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse FK without constraint name");
        assert_eq!(units.len(), 1);
        let sql = &units[0].sql;
        // Should use unnamed FK syntax (no CONSTRAINT name)
        assert!(
            sql.contains("ADD FOREIGN KEY (\"order_id\") REFERENCES \"orders\""),
            "Expected unnamed FK, got: {}",
            sql
        );
        assert!(
            !sql.contains("ADD CONSTRAINT"),
            "Expected no CONSTRAINT keyword for unnamed FK, got: {}",
            sql
        );
    }

    #[test]
    fn test_inline_fk_with_referenced_schema() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <createTable tableName="audit_log">
            <column name="user_id" type="uuid">
                <constraints nullable="false"
                             foreignKeyName="fk_audit_user"
                             referencedTableSchemaName="accounts"
                             referencedTableName="users"
                             referencedColumnNames="user_id"/>
            </column>
        </createTable>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse FK with referenced schema");
        assert_eq!(units.len(), 1);
        let sql = &units[0].sql;
        assert!(
            sql.contains("REFERENCES \"accounts\".\"users\""),
            "Expected schema-qualified reference, got: {}",
            sql
        );
    }

    #[test]
    fn test_map_liquibase_type_datetime() {
        assert_eq!(
            map_liquibase_type("datetime"),
            "timestamp without time zone"
        );
        assert_eq!(
            map_liquibase_type("DATETIME"),
            "timestamp without time zone"
        );
    }

    #[test]
    fn test_map_liquibase_type_with_modifiers() {
        assert_eq!(map_liquibase_type("decimal(10,2)"), "numeric(10,2)");
        assert_eq!(map_liquibase_type("number(5)"), "numeric(5)");
        assert_eq!(map_liquibase_type("nvarchar(255)"), "varchar(255)");
    }

    #[test]
    fn test_map_liquibase_type_passthrough() {
        // Types that PostgreSQL understands natively should pass through unchanged
        assert_eq!(map_liquibase_type("uuid"), "uuid");
        assert_eq!(map_liquibase_type("text"), "text");
        assert_eq!(map_liquibase_type("boolean"), "boolean");
        assert_eq!(map_liquibase_type("bigint"), "bigint");
        assert_eq!(map_liquibase_type("varchar(100)"), "varchar(100)");
        assert_eq!(map_liquibase_type("jsonb"), "jsonb");
    }

    #[test]
    fn test_map_liquibase_type_aliases() {
        assert_eq!(map_liquibase_type("int"), "integer");
        assert_eq!(map_liquibase_type("currency"), "numeric");
        assert_eq!(map_liquibase_type("clob"), "text");
        assert_eq!(map_liquibase_type("blob"), "bytea");
        assert_eq!(map_liquibase_type("tinyint"), "smallint");
        assert_eq!(map_liquibase_type("double"), "double precision");
    }

    #[test]
    fn test_inline_fk_skipped_without_referenced_columns() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <createTable tableName="child">
            <column name="parent_id" type="uuid">
                <constraints nullable="false"
                             foreignKeyName="fk_parent"
                             referencedTableName="parent"/>
            </column>
        </createTable>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse without referencedColumnNames");
        assert_eq!(units.len(), 1);
        // FK should be skipped (no referencedColumnNames), only CREATE TABLE emitted
        assert!(
            !units[0].sql.contains("FOREIGN KEY"),
            "Expected no FK without referencedColumnNames, got: {}",
            units[0].sql
        );
    }

    #[test]
    fn test_parse_references_attr() {
        assert_eq!(
            parse_references_attr("companies (id)"),
            Some(("companies".to_string(), "id".to_string()))
        );
        assert_eq!(
            parse_references_attr("sni_codes (sni_code)"),
            Some(("sni_codes".to_string(), "sni_code".to_string()))
        );
        assert_eq!(
            parse_references_attr(" table_name ( col1, col2 ) "),
            Some(("table_name".to_string(), "col1, col2".to_string()))
        );
        assert_eq!(parse_references_attr("no_parens"), None);
        assert_eq!(parse_references_attr("()"), None);
        assert_eq!(parse_references_attr("table ()"), None);
    }

    #[test]
    fn test_inline_fk_via_references_shorthand() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <createTable tableName="boards">
            <column name="id" type="uuid">
                <constraints nullable="false" primaryKey="true"/>
            </column>
            <column name="company_id" type="uuid">
                <constraints foreignKeyName="boards__fk__company_id"
                             references="companies (id)" nullable="false"/>
            </column>
        </createTable>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse references shorthand");
        assert_eq!(units.len(), 1);
        assert!(
            units[0].sql.contains("FOREIGN KEY"),
            "Expected FK from references shorthand, got: {}",
            units[0].sql
        );
        assert!(
            units[0].sql.contains("boards__fk__company_id"),
            "Expected FK constraint name, got: {}",
            units[0].sql
        );
        assert!(
            units[0].sql.contains("REFERENCES \"companies\""),
            "Expected REFERENCES companies, got: {}",
            units[0].sql
        );
    }

    #[test]
    fn test_inline_fk_via_references_multi_column() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <createTable tableName="parent">
            <column name="region" type="text">
                <constraints nullable="false"/>
            </column>
            <column name="id" type="integer">
                <constraints nullable="false"/>
            </column>
        </createTable>
        <createTable tableName="child">
            <column name="id" type="integer">
                <constraints nullable="false" primaryKey="true"/>
            </column>
            <column name="parent_region" type="text">
                <constraints foreignKeyName="fk_child_parent"
                             references="parent (region, id)" nullable="false"/>
            </column>
        </createTable>
    </changeSet>
</databaseChangeLog>"#;

        let units = parse_changelog_xml(xml, Path::new("test.xml"))
            .expect("Should parse multi-column references shorthand");
        let sql = &units[0].sql;
        assert!(
            sql.contains("FOREIGN KEY"),
            "Expected FK from multi-column references, got: {}",
            sql
        );
        assert!(
            sql.contains("REFERENCES \"parent\" (\"region\", \"id\")"),
            "Expected multi-column REFERENCES, got: {}",
            sql
        );
    }
}
