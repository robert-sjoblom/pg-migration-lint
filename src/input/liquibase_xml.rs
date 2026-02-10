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
//! - `<createIndex>` - generates CREATE INDEX SQL
//! - `<dropTable>` - generates DROP TABLE SQL
//! - `<dropIndex>` - generates DROP INDEX SQL
//! - `<addForeignKeyConstraint>` - generates ALTER TABLE ADD CONSTRAINT SQL
//! - `<addPrimaryKey>` - generates ALTER TABLE ADD CONSTRAINT ... PRIMARY KEY SQL
//! - `<addUniqueConstraint>` - generates ALTER TABLE ADD CONSTRAINT ... UNIQUE SQL
//!
//! Unknown change types are skipped with a SQL comment indicating they were
//! not processed, so the catalog can be flagged as potentially incomplete.

use crate::input::{LoadError, RawMigrationUnit};
use quick_xml::events::Event;
use quick_xml::Reader;
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
        let xml = std::fs::read_to_string(path).map_err(|e| LoadError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        let mut units = parse_changelog_xml(&xml, path)?;

        // Process <include> directives by loading referenced files
        let include_paths = extract_include_paths(&xml, path)?;
        for include_path in include_paths {
            let included_units = self.load(&include_path)?;
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
}

/// Information about the current changeSet being parsed.
#[derive(Debug, Clone)]
struct ChangeSetInfo {
    id: String,
    #[allow(dead_code)]
    author: String,
    run_in_transaction: bool,
    line: usize,
    sql_parts: Vec<String>,
}

impl ChangeSetInfo {
    /// Create a new ChangeSetInfo from attributes of a <changeSet> element.
    fn from_attributes(attrs: &[(String, String)], line: usize) -> Self {
        let id = get_attr(attrs, "id").unwrap_or_default();
        let author = get_attr(attrs, "author").unwrap_or_default();
        let run_in_transaction = get_attr(attrs, "runInTransaction")
            .map(|v| v != "false")
            .unwrap_or(true);

        Self {
            id,
            author,
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
}

/// Constraints parsed from a <constraints> element within a column.
#[derive(Debug, Clone)]
struct ColumnConstraints {
    nullable: bool,
    primary_key: bool,
    unique: bool,
}

impl Default for ColumnConstraints {
    /// Default constraints: nullable=true (standard SQL default), no PK, no unique.
    fn default() -> Self {
        Self {
            nullable: true,
            primary_key: false,
            unique: false,
        }
    }
}

/// Parse a Liquibase XML changelog into `RawMigrationUnit` entries.
///
/// Iterates through XML events using a state machine to track position
/// within the document structure. Each `<changeSet>` produces one
/// `RawMigrationUnit` with SQL generated from its child change elements.
fn parse_changelog_xml(xml: &str, source_path: &Path) -> Result<Vec<RawMigrationUnit>, LoadError> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);

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
                let current_line = byte_offset_to_line(xml, reader.buffer_position());

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
                let text = e.unescape().map_err(|err| LoadError::Parse {
                    path: source_path.to_path_buf(),
                    message: format!("XML text unescape error: {}", err),
                })?;
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
                    let table_name = get_attr(attrs, "tableName").unwrap_or_default();
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
                    let table_name = get_attr(attrs, "tableName").unwrap_or_default();
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
                    let table_name = get_attr(attrs, "tableName").unwrap_or_default();
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
                    let table_name = get_attr(attrs, "tableName").unwrap_or_default();
                    let schema_name = get_attr(attrs, "schemaName");
                    let qualified = qualify_name(&schema_name, &table_name);
                    cs.sql_parts.push(format!("DROP TABLE {};", qualified));
                    Ok(ParseState::InChangeSet(cs))
                }
                "dropIndex" => {
                    let index_name = get_attr(attrs, "indexName").unwrap_or_default();
                    let schema_name = get_attr(attrs, "schemaName");
                    let qualified = qualify_name(&schema_name, &index_name);
                    cs.sql_parts.push(format!("DROP INDEX {};", qualified));
                    Ok(ParseState::InChangeSet(cs))
                }
                "addForeignKeyConstraint" => {
                    let sql = generate_add_fk_sql(attrs);
                    cs.sql_parts.push(sql);
                    Ok(ParseState::InChangeSet(cs))
                }
                "addPrimaryKey" => {
                    let sql = generate_add_pk_sql(attrs);
                    cs.sql_parts.push(sql);
                    Ok(ParseState::InChangeSet(cs))
                }
                "addUniqueConstraint" => {
                    let sql = generate_add_unique_sql(attrs);
                    cs.sql_parts.push(sql);
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
                let name = get_attr(attrs, "name").unwrap_or_default();
                let type_name = get_attr(attrs, "type").unwrap_or_default();
                let default_value = get_attr(attrs, "defaultValue")
                    .or_else(|| get_attr(attrs, "defaultValueNumeric"))
                    .or_else(|| get_attr(attrs, "defaultValueBoolean"))
                    .or_else(|| get_attr(attrs, "defaultValueComputed"));

                if is_empty {
                    // Self-closing <column .../> with no constraints child
                    ct.columns.push(ColumnDef {
                        name,
                        type_name,
                        nullable: true,
                        primary_key: false,
                        unique: false,
                        default_value,
                    });
                    Ok(ParseState::InCreateTable(cs, ct))
                } else {
                    let col = ColumnState {
                        name,
                        type_name,
                        constraints: ColumnConstraints::default(),
                        default_value,
                    };
                    Ok(ParseState::InCreateTableColumn(cs, ct, col))
                }
            } else {
                // Unknown child of createTable, ignore
                Ok(ParseState::InCreateTable(cs, ct))
            }
        }
        ParseState::InCreateTableColumn(cs, ct, mut col) => {
            if tag_name == "constraints" {
                let nullable = get_attr(attrs, "nullable")
                    .map(|v| v != "false")
                    .unwrap_or(true);
                let primary_key = get_attr(attrs, "primaryKey")
                    .map(|v| v == "true")
                    .unwrap_or(false);
                let unique = get_attr(attrs, "unique")
                    .map(|v| v == "true")
                    .unwrap_or(false);
                col.constraints = ColumnConstraints {
                    nullable,
                    primary_key,
                    unique,
                };
            }
            Ok(ParseState::InCreateTableColumn(cs, ct, col))
        }
        ParseState::InAddColumn(cs, mut ac) => {
            if tag_name == "column" {
                let name = get_attr(attrs, "name").unwrap_or_default();
                let type_name = get_attr(attrs, "type").unwrap_or_default();
                let default_value = get_attr(attrs, "defaultValue")
                    .or_else(|| get_attr(attrs, "defaultValueNumeric"))
                    .or_else(|| get_attr(attrs, "defaultValueBoolean"))
                    .or_else(|| get_attr(attrs, "defaultValueComputed"));

                if is_empty {
                    ac.columns.push(ColumnDef {
                        name,
                        type_name,
                        nullable: true,
                        primary_key: false,
                        unique: false,
                        default_value,
                    });
                    Ok(ParseState::InAddColumn(cs, ac))
                } else {
                    let col_state = ColumnState {
                        name,
                        type_name,
                        constraints: ColumnConstraints::default(),
                        default_value,
                    };
                    Ok(ParseState::InAddColumnColumn(cs, ac, col_state))
                }
            } else {
                Ok(ParseState::InAddColumn(cs, ac))
            }
        }
        ParseState::InAddColumnColumn(cs, ac, mut col) => {
            if tag_name == "constraints" {
                let nullable = get_attr(attrs, "nullable")
                    .map(|v| v != "false")
                    .unwrap_or(true);
                let primary_key = get_attr(attrs, "primaryKey")
                    .map(|v| v == "true")
                    .unwrap_or(false);
                let unique = get_attr(attrs, "unique")
                    .map(|v| v == "true")
                    .unwrap_or(false);
                col.constraints = ColumnConstraints {
                    nullable,
                    primary_key,
                    unique,
                };
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
                let sql = generate_create_table_sql(&ct);
                cs.sql_parts.push(sql);
                Ok(ParseState::InChangeSet(cs))
            } else {
                Ok(ParseState::InCreateTable(cs, ct))
            }
        }
        ParseState::InCreateTableColumn(cs, mut ct, col) => {
            if tag_name == "column" {
                ct.columns.push(ColumnDef {
                    name: col.name,
                    type_name: col.type_name,
                    nullable: col.constraints.nullable,
                    primary_key: col.constraints.primary_key,
                    unique: col.constraints.unique,
                    default_value: col.default_value,
                });
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
                }
                Ok(ParseState::InChangeSet(cs))
            } else {
                Ok(ParseState::InAddColumn(cs, ac))
            }
        }
        ParseState::InAddColumnColumn(cs, mut ac, col) => {
            if tag_name == "column" {
                ac.columns.push(ColumnDef {
                    name: col.name,
                    type_name: col.type_name,
                    nullable: col.constraints.nullable,
                    primary_key: col.constraints.primary_key,
                    unique: col.constraints.unique,
                    default_value: col.default_value,
                });
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
// SQL generation helpers
// ---------------------------------------------------------------------------

/// Generate a CREATE TABLE SQL statement from parsed XML state.
fn generate_create_table_sql(ct: &CreateTableState) -> String {
    let qualified = qualify_name(&ct.schema_name, &ct.table_name);
    let mut parts: Vec<String> = Vec::new();

    for col in &ct.columns {
        let mut col_sql = format!("{} {}", col.name, col.type_name);

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

        parts.push(col_sql);
    }

    format!("CREATE TABLE {} ({});", qualified, parts.join(", "))
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
        qualified, col.name, col.type_name
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
    let columns = ci.columns.join(", ");

    format!(
        "CREATE {}INDEX {} ON {} ({});",
        unique_str, ci.index_name, table_qualified, columns
    )
}

/// Generate an ALTER TABLE ADD CONSTRAINT ... FOREIGN KEY SQL statement.
fn generate_add_fk_sql(attrs: &[(String, String)]) -> String {
    let constraint_name = get_attr(attrs, "constraintName").unwrap_or_default();
    let base_table = get_attr(attrs, "baseTableName").unwrap_or_default();
    let base_schema = get_attr(attrs, "baseTableSchemaName");
    let base_columns = get_attr(attrs, "baseColumnNames").unwrap_or_default();
    let ref_table = get_attr(attrs, "referencedTableName").unwrap_or_default();
    let ref_schema = get_attr(attrs, "referencedTableSchemaName");
    let ref_columns = get_attr(attrs, "referencedColumnNames").unwrap_or_default();

    let base_qualified = qualify_name(&base_schema, &base_table);
    let ref_qualified = qualify_name(&ref_schema, &ref_table);

    format!(
        "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({});",
        base_qualified, constraint_name, base_columns, ref_qualified, ref_columns
    )
}

/// Generate an ALTER TABLE ADD CONSTRAINT ... PRIMARY KEY SQL statement.
fn generate_add_pk_sql(attrs: &[(String, String)]) -> String {
    let constraint_name = get_attr(attrs, "constraintName").unwrap_or_default();
    let table_name = get_attr(attrs, "tableName").unwrap_or_default();
    let schema_name = get_attr(attrs, "schemaName");
    let column_names = get_attr(attrs, "columnNames").unwrap_or_default();

    let qualified = qualify_name(&schema_name, &table_name);

    if constraint_name.is_empty() {
        format!(
            "ALTER TABLE {} ADD PRIMARY KEY ({});",
            qualified, column_names
        )
    } else {
        format!(
            "ALTER TABLE {} ADD CONSTRAINT {} PRIMARY KEY ({});",
            qualified, constraint_name, column_names
        )
    }
}

/// Generate an ALTER TABLE ADD CONSTRAINT ... UNIQUE SQL statement.
fn generate_add_unique_sql(attrs: &[(String, String)]) -> String {
    let constraint_name = get_attr(attrs, "constraintName").unwrap_or_default();
    let table_name = get_attr(attrs, "tableName").unwrap_or_default();
    let schema_name = get_attr(attrs, "schemaName");
    let column_names = get_attr(attrs, "columnNames").unwrap_or_default();

    let qualified = qualify_name(&schema_name, &table_name);

    if constraint_name.is_empty() {
        format!("ALTER TABLE {} ADD UNIQUE ({});", qualified, column_names)
    } else {
        format!(
            "ALTER TABLE {} ADD CONSTRAINT {} UNIQUE ({});",
            qualified, constraint_name, column_names
        )
    }
}

// ---------------------------------------------------------------------------
// Utility helpers
// ---------------------------------------------------------------------------

/// Qualify a name with an optional schema prefix.
///
/// Returns `schema.name` if schema is present, or just `name` otherwise.
fn qualify_name(schema: &Option<String>, name: &str) -> String {
    match schema {
        Some(s) if !s.is_empty() => format!("{}.{}", s, name),
        _ => name.to_string(),
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

/// Extract `<include file="..."/>` paths from the XML, resolved relative to the parent file.
fn extract_include_paths(xml: &str, source_path: &Path) -> Result<Vec<PathBuf>, LoadError> {
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);
    let mut buf = Vec::new();
    let mut paths = Vec::new();

    let parent_dir = source_path.parent().unwrap_or_else(|| Path::new("."));

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| LoadError::Parse {
                path: source_path.to_path_buf(),
                message: format!("XML parse error while scanning includes: {}", e),
            })?;

        match event {
            Event::Eof => break,
            Event::Start(ref e) | Event::Empty(ref e) => {
                let tag_name = local_name_str(e.name().as_ref());
                if tag_name == "include" {
                    let attrs = collect_attributes(e)?;
                    if let Some(file_attr) = get_attr(&attrs, "file") {
                        let resolved = parent_dir.join(&file_attr);
                        paths.push(resolved);
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
            sql.contains("CREATE TABLE users"),
            "Expected CREATE TABLE, got: {}",
            sql
        );
        assert!(
            sql.contains("id integer PRIMARY KEY NOT NULL"),
            "Expected PK column, got: {}",
            sql
        );
        assert!(
            sql.contains("name varchar(100) NOT NULL"),
            "Expected NOT NULL column, got: {}",
            sql
        );
        assert!(
            sql.contains("email varchar(255)"),
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
            sql.contains("ALTER TABLE users ADD COLUMN age integer NOT NULL;"),
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
            sql.contains("CREATE UNIQUE INDEX idx_users_email ON users (email);"),
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
        assert!(units[0].sql.contains("CREATE TABLE users"));

        assert_eq!(units[1].id, "2");
        assert!(units[1].sql.contains("CREATE INDEX idx_users_name"));

        assert_eq!(units[2].id, "3");
        assert_eq!(units[2].sql, "DROP TABLE old_users;");
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
        assert_eq!(units[0].sql, "DROP TABLE old_table;");
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
        assert_eq!(units[0].sql, "DROP INDEX idx_old;");
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
            sql.contains("ALTER TABLE orders ADD CONSTRAINT fk_order_user FOREIGN KEY (user_id) REFERENCES users (id);"),
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
            sql.contains("ALTER TABLE orders ADD CONSTRAINT pk_orders PRIMARY KEY (id);"),
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
            sql.contains("ALTER TABLE users ADD CONSTRAINT uq_users_email UNIQUE (email);"),
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
            units[0].sql.contains("CREATE TABLE myschema.users"),
            "Expected schema-qualified name, got: {}",
            units[0].sql
        );
    }

    #[test]
    fn test_unknown_change_type_generates_comment() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<databaseChangeLog xmlns="http://www.liquibase.org/xml/ns/dbchangelog">
    <changeSet id="1" author="dev">
        <modifyDataType tableName="users" columnName="name" newDataType="text"/>
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
        assert!(units[0].sql.contains("CREATE TABLE users"));
        assert!(units[0]
            .sql
            .contains("CREATE INDEX idx_users_id ON users (id);"));
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
                .contains("CREATE TABLE simple (id integer, name text);"),
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
            units[0]
                .sql
                .contains("CREATE INDEX idx_orders_composite ON orders (user_id, created_at);"),
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
            sql.contains("ALTER TABLE sales.orders ADD CONSTRAINT fk_order_user FOREIGN KEY (user_id) REFERENCES accounts.users (id);"),
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
        assert_eq!(units[0].sql, "ALTER TABLE orders ADD PRIMARY KEY (id);");
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
        assert_eq!(qualify_name(&None, "users"), "users");
        assert_eq!(
            qualify_name(&Some("public".to_string()), "users"),
            "public.users"
        );
        assert_eq!(qualify_name(&Some("".to_string()), "users"), "users");
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

        let paths = extract_include_paths(xml, Path::new("/project/db/main.xml"))
            .expect("Should extract include paths");
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0], PathBuf::from("/project/db/sub/changelog-1.xml"));
        assert_eq!(paths[1], PathBuf::from("/project/db/sub/changelog-2.xml"));
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
}
