// --- Scratch snapshot scaffold for the schema-scoped-index-collision investigation ---
//
// See TODO.md "Design: per-schema `SchemaState` catalog structure". These tests parse real
// SQL "steps" (one step per would-be migration file) through the actual parser + normalizer,
// then snapshot catalog state after each step so the collision bug is directly visible in the
// snapshot diffs. They document TODAY's (buggy) behavior — not the desired end state — and are
// meant to be deleted once the SchemaState rework lands and real path-based assertions replace
// them.

use std::path::PathBuf;

use crate::{Catalog, TableState, catalog::replay::apply, input::MigrationUnit};

/// Parse each SQL string into its own `MigrationUnit` (mirrors one migration file each),
/// then normalize the whole batch, matching the real pipeline's load-then-normalize order.
fn steps_from_sql(sql_steps: &[&str], default_schema: &str) -> Vec<MigrationUnit> {
    let mut units: Vec<MigrationUnit> = sql_steps
        .iter()
        .enumerate()
        .map(|(i, sql)| MigrationUnit {
            id: format!("step_{i}"),
            statements: crate::parser::pg_query::parse_sql(sql),
            source_file: PathBuf::from(format!("step_{i}.sql")),
            source_line_offset: 1,
            run_in_transaction: true,
            is_down: false,
        })
        .collect();
    crate::normalize::normalize_schemas(&mut units, default_schema);
    units
}

/// Deterministic, human-readable catalog dump for snapshotting.
///
/// Shows each table's own index list (always correct — no ambiguity within one table) next to
/// the global `index_to_table` reverse-lookup result for every bare index name seen (today's
/// single flat map — this is where the cross-schema collision actually lives). A mismatch
/// between a table's own listed index and what the lookup section resolves to for that same
/// name *is* the bug.
fn snapshot_catalog(catalog: &Catalog) -> String {
    let mut tables: Vec<&TableState> = catalog.tables().collect();
    tables.sort_by(|a, b| a.name.cmp(&b.name));

    let mut all_index_names = std::collections::BTreeSet::new();
    let mut out = String::from("tables:\n");
    for table in &tables {
        let mut index_names: Vec<&str> = table.indexes.iter().map(|i| i.name.as_str()).collect();
        index_names.sort_unstable();
        all_index_names.extend(index_names.iter().map(|s| s.to_string()));
        let indexes = if index_names.is_empty() {
            "none".to_string()
        } else {
            index_names.join(", ")
        };
        out.push_str(&format!("  {} (indexes: {indexes})\n", table.name));
    }

    out.push_str("index_to_table lookups:\n");
    for name in &all_index_names {
        let owner = catalog.table_for_index(name).unwrap_or("<none>");
        out.push_str(&format!("  {name} -> {owner}\n"));
    }
    out
}

/// Idempotent `{:#?}` dump of full catalog state (all columns/indexes/constraints,
/// not just the summary `snapshot_catalog` above). `TableState` etc. already derive
/// `Debug`; the only nondeterminism is `Catalog::tables()`'s `HashMap` iteration
/// order, fixed here by sorting the collected `Vec` before formatting.
fn debug_snapshot_catalog(catalog: &Catalog) -> String {
    let mut tables: Vec<&TableState> = catalog.tables().collect();
    tables.sort_by(|a, b| a.name.cmp(&b.name));
    format!("{tables:#?}")
}

/// Replay each step in order, snapshotting catalog state after every one.
fn snapshot_each_step(snapshot_name_prefix: &str, units: &[MigrationUnit]) {
    let mut catalog = Catalog::new();
    for (i, unit) in units.iter().enumerate() {
        apply(&mut catalog, unit);
        insta::assert_snapshot!(
            format!("{snapshot_name_prefix}_step_{i}"),
            snapshot_catalog(&catalog)
        );
        insta::assert_snapshot!(
            format!("{snapshot_name_prefix}_step_{i}_debug"),
            debug_snapshot_catalog(&catalog)
        );
    }
}

#[test]
fn snapshot_index_collision_both_schemas_qualified() {
    // analytics.orders and data.orders both explicitly qualified; each gets an
    // index named idx_customer. Fully unambiguous SQL — the collision is purely
    // a catalog bug (flat, schema-blind index_to_table reverse map).
    let units = steps_from_sql(
        &[
            "CREATE TABLE analytics.orders (id integer PRIMARY KEY);
             CREATE TABLE data.orders (id integer PRIMARY KEY);",
            "CREATE INDEX idx_customer ON analytics.orders (id);",
            "CREATE INDEX idx_customer ON data.orders (id);",
        ],
        "public",
    );
    snapshot_each_step("both_schemas_qualified", &units);
}

#[test]
fn snapshot_index_collision_neither_schema_qualified() {
    // Two different, unrelated tables, neither schema-qualified, so both normalize
    // into the SAME default schema ("public"). A same-named index across them is a
    // same-schema collision — arguably invalid SQL Postgres itself would reject —
    // not a cross-schema case. Included to document what the catalog does with it
    // today (see apply_create_index's "overwrites existing index" warning).
    let units = steps_from_sql(
        &[
            "CREATE TABLE metrics (id integer PRIMARY KEY);
             CREATE TABLE events (id integer PRIMARY KEY);",
            "CREATE INDEX idx_created_at ON metrics (id);",
            "CREATE INDEX idx_created_at ON events (id);",
        ],
        "public",
    );
    snapshot_each_step("neither_schema_qualified", &units);
}

#[test]
fn snapshot_index_collision_one_schema_qualified() {
    // analytics.orders is explicitly qualified; the second `orders` is unqualified
    // and normalizes to public.orders. Two genuinely different schemas (analytics
    // vs public) — same collision as the fully-qualified case, showing that
    // default-schema normalization doesn't save you.
    let units = steps_from_sql(
        &[
            "CREATE TABLE analytics.orders (id integer PRIMARY KEY);
             CREATE TABLE orders (id integer PRIMARY KEY);",
            "CREATE INDEX idx_customer ON analytics.orders (id);",
            "CREATE INDEX idx_customer ON orders (id);",
        ],
        "public",
    );
    snapshot_each_step("one_schema_qualified", &units);
}
