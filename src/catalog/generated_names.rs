//! Names Postgres generates for constraints and indexes created without one.
//!
//! A port of `makeObjectName`, `ChooseIndexColumnNames`,
//! `ChooseIndexNameAddition` and `ChooseForeignKeyConstraintNameAddition`
//! (`src/backend/commands/indexcmds.c` and `tablecmds.c`). The behaviour is
//! the one measured on PG 14 to 19 in `verify/tests/naming/*.sql`.
//!
//! Collision suffixes are not modelled. When the generated name is already
//! taken in the schema, Postgres retries with a numbered label (`key1`,
//! `key2`, ...), which depends on every constraint and relation in the
//! schema. The catalog has no per-schema state yet, so
//! [`GeneratedName::unsuffixed`] always returns the first choice. A suffixed
//! name is [`make_object_name`] called with the numbered label.

use crate::parser::ir::{CreateIndex, IndexColumn};

/// `NAMEDATALEN - 1`: the longest identifier Postgres stores, in bytes.
const MAX_IDENTIFIER_BYTES: usize = 63;

/// `NAMEDATALEN`: once the joined column list reaches this many bytes,
/// `ChooseIndexNameAddition` stops appending column names.
const NAME_ADDITION_CAP_BYTES: usize = MAX_IDENTIFIER_BYTES + 1;

/// The label that ends a generated name and identifies the kind of object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameLabel {
    PrimaryKey,
    Unique,
    ForeignKey,
    Check,
    Exclude,
    Index,
}

impl NameLabel {
    fn as_str(self) -> &'static str {
        match self {
            Self::PrimaryKey => "pkey",
            Self::Unique => "key",
            Self::ForeignKey => "fkey",
            Self::Check => "check",
            Self::Exclude => "excl",
            Self::Index => "idx",
        }
    }
}

/// The parts Postgres builds a generated constraint or index name from: the
/// owning table's bare name, an optional column part, and a label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GeneratedName {
    table: String,
    column_part: Option<String>,
    label: NameLabel,
}

impl GeneratedName {
    /// An unnamed PRIMARY KEY: `{table}_pkey`. The key and INCLUDE columns
    /// never appear in the name.
    pub(crate) fn primary_key(table: &str) -> Self {
        Self::new(table, None, NameLabel::PrimaryKey)
    }

    /// An unnamed UNIQUE constraint: `{table}_{columns}_key`, with the
    /// INCLUDE columns appended to the key columns.
    pub(crate) fn unique(table: &str, columns: &[String], include_columns: &[String]) -> Self {
        let column_part = index_column_part(columns.iter().map(String::as_str), include_columns);
        Self::new(table, Some(column_part), NameLabel::Unique)
    }

    /// An unnamed FOREIGN KEY: `{table}_{referencing columns}_fkey`. The
    /// referenced table and columns never appear in the name.
    pub(crate) fn foreign_key(table: &str, columns: &[String]) -> Self {
        let column_part = join_name_addition(columns.iter().map(String::as_str));
        Self::new(table, Some(column_part), NameLabel::ForeignKey)
    }

    /// An unnamed CHECK: `{table}_{column}_check` when the expression
    /// references exactly one distinct column, `{table}_check` otherwise.
    /// `referenced_columns` must already be distinct, as the IR provides it.
    pub(crate) fn check(table: &str, referenced_columns: &[String]) -> Self {
        let column_part = match referenced_columns {
            [only] => Some(only.clone()),
            _ => None,
        };
        Self::new(table, column_part, NameLabel::Check)
    }

    /// An unnamed EXCLUDE constraint: `{table}_{elements}_excl`, each element
    /// contributing its [`IndexColumn::generated_name_part`], with the
    /// INCLUDE columns appended.
    pub(crate) fn exclude(
        table: &str,
        elements: &[IndexColumn],
        include_columns: &[String],
    ) -> Self {
        let column_part = index_column_part(
            elements.iter().map(IndexColumn::generated_name_part),
            include_columns,
        );
        Self::new(table, Some(column_part), NameLabel::Exclude)
    }

    /// An unnamed `CREATE INDEX`: `{table}_{elements}_idx`, each element
    /// contributing its [`IndexColumn::generated_name_part`], with the
    /// INCLUDE columns appended. `UNIQUE`, `USING`, `WHERE`, sort order,
    /// opclass, `COLLATE` and `CONCURRENTLY` do not change the name.
    pub(crate) fn index(table: &str, elements: &[IndexColumn], include_columns: &[String]) -> Self {
        let column_part = index_column_part(
            elements.iter().map(IndexColumn::generated_name_part),
            include_columns,
        );
        Self::new(table, Some(column_part), NameLabel::Index)
    }

    fn new(table: &str, column_part: Option<String>, label: NameLabel) -> Self {
        Self {
            table: table.to_string(),
            column_part,
            label,
        }
    }

    /// The name Postgres picks when no object in the schema already has it,
    /// cut to 63 bytes. See the module docs for the collision suffix this
    /// leaves out.
    pub(crate) fn unsuffixed(&self) -> String {
        make_object_name(
            &self.table,
            self.column_part.as_deref(),
            self.label.as_str(),
        )
    }
}

/// The name of the index a `CREATE INDEX` creates: its own name, else the
/// [`GeneratedName::index`] for its bare table name, without a collision
/// suffix.
pub(crate) fn created_index_name(ci: &CreateIndex) -> String {
    match &ci.index_name {
        Some(name) => name.clone(),
        None => GeneratedName::index(ci.table_name.name(), &ci.columns, &ci.include_columns)
            .unsuffixed(),
    }
}

/// `makeObjectName`: join `name1`, `name2` and `label` with `_`, cutting
/// `name1` and `name2` so the result fits in 63 bytes. The label is never cut.
///
/// While over budget, the longer of the two names loses a byte, and on a tie
/// `name2` loses it. Each name is then clipped back to a whole character, so
/// a multibyte name can leave the result under 63 bytes.
fn make_object_name(name1: &str, name2: Option<&str>, label: &str) -> String {
    let separators = if name2.is_some() { 2 } else { 1 };
    let budget = MAX_IDENTIFIER_BYTES.saturating_sub(label.len() + separators);

    let mut name1_bytes = name1.len();
    let mut name2_bytes = name2.map_or(0, str::len);
    while name1_bytes + name2_bytes > budget {
        if name1_bytes > name2_bytes {
            name1_bytes -= 1;
        } else {
            name2_bytes -= 1;
        }
    }

    let mut name = name1[..name1.floor_char_boundary(name1_bytes)].to_string();
    if let Some(name2) = name2 {
        name.push('_');
        name.push_str(&name2[..name2.floor_char_boundary(name2_bytes)]);
    }
    name.push('_');
    name.push_str(label);
    name
}

/// The column part of an index or index-backed constraint name: the
/// element name parts followed by the INCLUDE columns, made distinct by
/// `ChooseIndexColumnNames`, then joined by `ChooseIndexNameAddition`.
fn index_column_part<'a>(
    element_name_parts: impl IntoIterator<Item = &'a str>,
    include_columns: &'a [String],
) -> String {
    let distinct = choose_index_column_names(
        element_name_parts
            .into_iter()
            .chain(include_columns.iter().map(String::as_str)),
    );
    join_name_addition(distinct.iter().map(String::as_str))
}

/// `ChooseIndexColumnNames`: a part equal to any earlier chosen part gets
/// the first free number appended (`a, a` gives `a, a1`), the original part
/// first clipped so the result stays within 63 bytes.
fn choose_index_column_names<'a>(name_parts: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut chosen: Vec<String> = Vec::new();
    for original in name_parts {
        let mut candidate = original.to_string();
        let mut number = 1usize;
        while chosen.contains(&candidate) {
            let digits = number.to_string();
            let kept = original.floor_char_boundary(MAX_IDENTIFIER_BYTES - digits.len());
            candidate = format!("{}{digits}", &original[..kept]);
            number += 1;
        }
        chosen.push(candidate);
    }
    chosen
}

/// `ChooseIndexNameAddition` and `ChooseForeignKeyConstraintNameAddition`,
/// which are identical: join the names with `_`, appending no further name
/// once the result has reached `NAMEDATALEN` bytes.
///
/// The cap can't change a generated name: [`make_object_name`] keeps at
/// most 58 bytes of it either way. Three 30-byte columns are all appended
/// (the list is 61 bytes before the third), and the cut happens there.
fn join_name_addition<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    let mut addition = String::new();
    for name in names {
        if !addition.is_empty() {
            addition.push('_');
        }
        addition.push_str(&name[..name.floor_char_boundary(MAX_IDENTIFIER_BYTES)]);
        if addition.len() >= NAME_ADDITION_CAP_BYTES {
            break;
        }
    }
    addition
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn column(name: &str) -> IndexColumn {
        IndexColumn::Column(name.to_string())
    }

    fn expression(generated_name_part: &str) -> IndexColumn {
        IndexColumn::Expression {
            text: format!("<{generated_name_part} expression>"),
            referenced_columns: vec![],
            generated_name_part: generated_name_part.to_string(),
        }
    }

    #[rstest]
    #[case::pk_inline("pk_inline", "pk_inline_pkey")]
    #[case::pk_table_level("pk_table_level", "pk_table_level_pkey")]
    #[case::pk_composite("pk_composite", "pk_composite_pkey")]
    #[case::pk_include("pk_include", "pk_include_pkey")]
    #[case::pk_add_column("pk_add_column", "pk_add_column_pkey")]
    #[case::pk_add_pk("pk_add_pk", "pk_add_pk_pkey")]
    #[case::schema_qualified_table_uses_bare_name("orders", "orders_pkey")]
    #[case::partitioned_parent("ev", "ev_pkey")]
    #[case::quoted_identifier_keeps_case("Orders", "Orders_pkey")]
    #[case::unquoted_identifier_folded_by_parser("mixedcase", "mixedcase_pkey")]
    fn primary_key_is_table_pkey(#[case] table: &str, #[case] expected: &str) {
        assert_eq!(GeneratedName::primary_key(table).unsuffixed(), expected);
    }

    #[rstest]
    #[case::uq_inline("uq_inline", &["a"], &[], "uq_inline_a_key")]
    #[case::uq_table_level("uq_table_level", &["a"], &[], "uq_table_level_a_key")]
    #[case::uq_multi("uq_multi", &["a", "b"], &[], "uq_multi_a_b_key")]
    #[case::uq_order_follows_constraint_order("uq_order", &["c", "a"], &[], "uq_order_c_a_key")]
    #[case::uq_include_appended("uq_include", &["a"], &["b"], "uq_include_a_b_key")]
    #[case::uq_alter("uq_alter", &["a", "b"], &[], "uq_alter_a_b_key")]
    #[case::uq_add_column("uq_add_column", &["a"], &[], "uq_add_column_a_key")]
    #[case::partitioned_parent("ev", &["u", "ts"], &[], "ev_u_ts_key")]
    #[case::quoted_identifiers_keep_case("Orders", &["Code"], &[], "Orders_Code_key")]
    #[case::quoted_spaces_carry_over("order lines", &["line no"], &[], "order lines_line no_key")]
    fn unique_is_table_columns_key(
        #[case] table: &str,
        #[case] columns: &[&str],
        #[case] include_columns: &[&str],
        #[case] expected: &str,
    ) {
        let name = GeneratedName::unique(table, &strings(columns), &strings(include_columns));
        assert_eq!(name.unsuffixed(), expected);
    }

    #[rstest]
    #[case::fk_inline("fk_inline", &["parent_id"], "fk_inline_parent_id_fkey")]
    #[case::fk_table_level("fk_table_level", &["pid"], "fk_table_level_pid_fkey")]
    #[case::fk_multi_in_fk_order("fk_multi", &["pid", "px"], "fk_multi_pid_px_fkey")]
    #[case::fk_alter("fk_alter", &["pid"], "fk_alter_pid_fkey")]
    #[case::fk_add_column("fk_add_column", &["pid"], "fk_add_column_pid_fkey")]
    #[case::partitioned_parent("ev", &["customer_id"], "ev_customer_id_fkey")]
    fn foreign_key_is_table_referencing_columns_fkey(
        #[case] table: &str,
        #[case] columns: &[&str],
        #[case] expected: &str,
    ) {
        assert_eq!(
            GeneratedName::foreign_key(table, &strings(columns)).unsuffixed(),
            expected
        );
    }

    #[rstest]
    #[case::ck_column("ck_column", &["a"], "ck_column_a_check")]
    #[case::ck_table_one("ck_table_one", &["b"], "ck_table_one_b_check")]
    #[case::ck_table_two("ck_table_two", &["a", "b"], "ck_table_two_check")]
    #[case::ck_table_none("ck_table_none", &[], "ck_table_none_check")]
    #[case::ck_column_other("ck_column_other", &["b", "a"], "ck_column_other_check")]
    #[case::ck_column_foreign("ck_column_foreign", &["a"], "ck_column_foreign_a_check")]
    #[case::ck_column_none("ck_column_none", &[], "ck_column_none_check")]
    #[case::ck_repeat("ck_repeat", &["a"], "ck_repeat_a_check")]
    #[case::ck_function("ck_function", &["a"], "ck_function_a_check")]
    #[case::ck_between("ck_between", &["a", "b", "c"], "ck_between_check")]
    #[case::ck_in_list("ck_in_list", &["a"], "ck_in_list_a_check")]
    #[case::ck_boolean_test("ck_boolean_test", &["flag"], "ck_boolean_test_flag_check")]
    #[case::ck_subscript("ck_subscript", &["arr"], "ck_subscript_arr_check")]
    #[case::ck_alter_one_column("ck_alter", &["a"], "ck_alter_a_check")]
    #[case::ck_alter_two_columns("ck_alter", &["a", "b"], "ck_alter_check")]
    #[case::ck_add_column_own_column("ck_add_column", &["b"], "ck_add_column_b_check")]
    #[case::ck_add_column_other_column("ck_add_column", &["c", "a"], "ck_add_column_check")]
    #[case::ck_is_json("ck_is_json", &["doc"], "ck_is_json_doc_check")]
    #[case::ck_is_json_two("ck_is_json_two", &["doc", "alt"], "ck_is_json_two_check")]
    #[case::partitioned_parent("ev", &["c"], "ev_c_check")]
    #[case::quoted_identifiers_keep_case("Orders", &["Qty"], "Orders_Qty_check")]
    fn check_names_a_single_referenced_column_only(
        #[case] table: &str,
        #[case] referenced_columns: &[&str],
        #[case] expected: &str,
    ) {
        assert_eq!(
            GeneratedName::check(table, &strings(referenced_columns)).unsuffixed(),
            expected
        );
    }

    #[rstest]
    #[case::ex_btree("ex_btree", vec![column("a")], &[], "ex_btree_a_excl")]
    #[case::ex_gist("ex_gist", vec![column("r")], &[], "ex_gist_r_excl")]
    #[case::ex_multi("ex_multi", vec![column("a"), column("b")], &[], "ex_multi_a_b_excl")]
    #[case::ex_function("ex_function", vec![expression("lower")], &[], "ex_function_lower_excl")]
    #[case::ex_operator("ex_operator", vec![expression("expr")], &[], "ex_operator_expr_excl")]
    #[case::ex_include_appended("ex_include", vec![column("a")], &["b"], "ex_include_a_b_excl")]
    #[case::ex_where("ex_where", vec![column("a")], &[], "ex_where_a_excl")]
    #[case::ex_alter("ex_alter", vec![column("a")], &[], "ex_alter_a_excl")]
    fn exclude_is_table_elements_excl(
        #[case] table: &str,
        #[case] elements: Vec<IndexColumn>,
        #[case] include_columns: &[&str],
        #[case] expected: &str,
    ) {
        let name = GeneratedName::exclude(table, &elements, &strings(include_columns));
        assert_eq!(name.unsuffixed(), expected);
    }

    #[rstest]
    #[case::i_single("i_single", vec![column("a")], &[], "i_single_a_idx")]
    #[case::i_multi_in_index_order("i_multi", vec![column("c"), column("a")], &[], "i_multi_c_a_idx")]
    #[case::i_qualified_uses_bare_table("i_qualified", vec![column("a")], &[], "i_qualified_a_idx")]
    #[case::i_lower("i_lower", vec![expression("lower")], &[], "i_lower_lower_idx")]
    #[case::i_qualified_fn("i_qualified_fn", vec![expression("lower")], &[], "i_qualified_fn_lower_idx")]
    #[case::i_coalesce("i_coalesce", vec![expression("coalesce")], &[], "i_coalesce_coalesce_idx")]
    #[case::i_case("i_case", vec![expression("case")], &[], "i_case_case_idx")]
    #[case::i_plus("i_plus", vec![expression("expr")], &[], "i_plus_expr_idx")]
    #[case::i_jsonb("i_jsonb", vec![expression("expr")], &[], "i_jsonb_expr_idx")]
    #[case::i_mixed_column_then_function(
        "i_mixed",
        vec![column("a"), expression("lower")],
        &[],
        "i_mixed_a_lower_idx"
    )]
    #[case::i_mixed_function_operator_column(
        "i_mixed",
        vec![expression("lower"), expression("expr"), column("b")],
        &[],
        "i_mixed_lower_expr_b_idx"
    )]
    #[case::i_cast_column("i_cast_column", vec![expression("a")], &[], "i_cast_column_a_idx")]
    #[case::i_cast_expr_text("i_cast_expr", vec![expression("text")], &[], "i_cast_expr_text_idx")]
    #[case::i_cast_expr_int8("i_cast_expr", vec![expression("int8")], &[], "i_cast_expr_int8_idx")]
    #[case::i_cast_expr_int4("i_cast_expr", vec![expression("int4")], &[], "i_cast_expr_int4_idx")]
    #[case::i_cast_function_lower(
        "i_cast_function",
        vec![expression("lower")],
        &[],
        "i_cast_function_lower_idx"
    )]
    #[case::i_cast_function_coalesce(
        "i_cast_function",
        vec![expression("coalesce")],
        &[],
        "i_cast_function_coalesce_idx"
    )]
    #[case::i_paren_column("i_paren_column", vec![expression("a")], &[], "i_paren_column_a_idx")]
    #[case::i_minmax_greatest("i_minmax", vec![expression("greatest")], &[], "i_minmax_greatest_idx")]
    #[case::i_minmax_least("i_minmax", vec![expression("least")], &[], "i_minmax_least_idx")]
    #[case::i_nullif("i_nullif", vec![expression("nullif")], &[], "i_nullif_nullif_idx")]
    #[case::i_array("i_array", vec![expression("array")], &[], "i_array_array_idx")]
    #[case::i_case_else_column("i_case_else", vec![expression("b")], &[], "i_case_else_b_idx")]
    #[case::i_case_else_function("i_case_else", vec![expression("abs")], &[], "i_case_else_abs_idx")]
    #[case::i_case_else_cast_expression(
        "i_case_else",
        vec![expression("case")],
        &[],
        "i_case_else_case_idx"
    )]
    #[case::i_cast_case("i_cast_case", vec![expression("text")], &[], "i_cast_case_text_idx")]
    #[case::i_collate_expr_column(
        "i_collate_expr",
        vec![expression("email")],
        &[],
        "i_collate_expr_email_idx"
    )]
    #[case::i_collate_expr_function(
        "i_collate_expr",
        vec![expression("lower")],
        &[],
        "i_collate_expr_lower_idx"
    )]
    #[case::i_collate_expr_cast(
        "i_collate_expr",
        vec![expression("text")],
        &[],
        "i_collate_expr_text_idx"
    )]
    #[case::i_indirection_array_subscript(
        "i_indirection",
        vec![expression("arr")],
        &[],
        "i_indirection_arr_idx"
    )]
    #[case::i_indirection_jsonb_subscript(
        "i_indirection",
        vec![expression("js")],
        &[],
        "i_indirection_js_idx"
    )]
    #[case::i_indirection_field_selection(
        "i_indirection",
        vec![expression("x")],
        &[],
        "i_indirection_x_idx"
    )]
    #[case::i_null_test("i_null_test", vec![expression("expr")], &[], "i_null_test_expr_idx")]
    #[case::i_bool_expr("i_bool_expr", vec![expression("expr")], &[], "i_bool_expr_expr_idx")]
    #[case::i_partial("i_partial", vec![column("a")], &[], "i_partial_a_idx")]
    #[case::i_unique_keeps_idx_label("i_unique", vec![column("a")], &[], "i_unique_a_idx")]
    #[case::i_include_appended("i_include", vec![column("a")], &["b"], "i_include_a_b_idx")]
    #[case::i_repeat_column("i_repeat", vec![column("a"), column("a")], &[], "i_repeat_a_a1_idx")]
    #[case::i_repeat_function(
        "i_repeat",
        vec![expression("lower"), expression("lower")],
        &[],
        "i_repeat_lower_lower1_idx"
    )]
    #[case::i_hash("i_hash", vec![column("a")], &[], "i_hash_a_idx")]
    #[case::i_desc("i_desc", vec![column("a")], &[], "i_desc_a_idx")]
    #[case::i_opclass("i_opclass", vec![column("a")], &[], "i_opclass_a_idx")]
    #[case::i_collate("i_collate", vec![column("a")], &[], "i_collate_a_idx")]
    #[case::i_concurrently("i_concurrently", vec![column("a")], &[], "i_concurrently_a_idx")]
    #[case::partitioned_parent("ev", vec![column("c")], &[], "ev_c_idx")]
    #[case::quoted_identifiers_keep_case("Orders", vec![column("Qty")], &[], "Orders_Qty_idx")]
    fn index_is_table_elements_idx(
        #[case] table: &str,
        #[case] elements: Vec<IndexColumn>,
        #[case] include_columns: &[&str],
        #[case] expected: &str,
    ) {
        let name = GeneratedName::index(table, &elements, &strings(include_columns));
        assert_eq!(name.unsuffixed(), expected);
    }

    #[test]
    fn repeated_element_counts_against_every_earlier_choice() {
        let elements = [column("a"), column("a1"), column("a")];
        assert_eq!(
            GeneratedName::index("t", &elements, &[]).unsuffixed(),
            "t_a_a1_a2_idx"
        );
    }

    #[test]
    fn include_column_repeating_a_key_column_is_numbered() {
        let name = GeneratedName::index("t", &[column("a")], &strings(&["a"]));
        assert_eq!(name.unsuffixed(), "t_a_a1_idx");
    }

    #[test]
    fn column_list_stops_growing_once_it_reaches_namedatalen() {
        let thirty = [
            "a23456789012345678901234567890",
            "b23456789012345678901234567890",
            "c23456789012345678901234567890",
            "d23456789012345678901234567890",
        ];
        assert_eq!(
            join_name_addition(thirty),
            "a23456789012345678901234567890_b23456789012345678901234567890_c23456789012345678901234567890"
        );
    }

    #[test]
    fn truncation_pk_on_63_char_table_keeps_58_chars() {
        let name = GeneratedName::primary_key(
            "p23456789012345678901234567890123456789012345678901234567890123",
        );
        assert_eq!(
            name.unsuffixed(),
            "p234567890123456789012345678901234567890123456789012345678_pkey"
        );
    }

    #[test]
    fn truncation_unique_on_63_char_table_keeps_57_chars() {
        let name = GeneratedName::unique(
            "u23456789012345678901234567890123456789012345678901234567890123",
            &strings(&["a"]),
            &[],
        );
        assert_eq!(
            name.unsuffixed(),
            "u23456789012345678901234567890123456789012345678901234567_a_key"
        );
    }

    #[test]
    fn truncation_check_on_63_char_table_keeps_55_chars() {
        let name = GeneratedName::check(
            "k23456789012345678901234567890123456789012345678901234567890123",
            &strings(&["b"]),
        );
        assert_eq!(
            name.unsuffixed(),
            "k234567890123456789012345678901234567890123456789012345_b_check"
        );
    }

    #[test]
    fn truncation_63_char_column_on_short_table_keeps_53_chars() {
        let name = GeneratedName::unique(
            "short",
            &strings(&["c23456789012345678901234567890123456789012345678901234567890123"]),
            &[],
        );
        assert_eq!(
            name.unsuffixed(),
            "short_c2345678901234567890123456789012345678901234567890123_key"
        );
    }

    #[test]
    fn truncation_cuts_only_the_longer_table_when_that_suffices() {
        let name = GeneratedName::unique(
            "w2345678901234567890123456789012345678901234567890",
            &strings(&["customer_id"]),
            &[],
        );
        assert_eq!(
            name.unsuffixed(),
            "w2345678901234567890123456789012345678901234567_customer_id_key"
        );
    }

    #[test]
    fn truncation_cuts_two_63_char_parts_to_29_each() {
        let name = GeneratedName::unique(
            "x23456789012345678901234567890123456789012345678901234567890123",
            &strings(&["y23456789012345678901234567890123456789012345678901234567890123"]),
            &[],
        );
        assert_eq!(
            name.unsuffixed(),
            "x2345678901234567890123456789_y2345678901234567890123456789_key"
        );
    }

    #[test]
    fn truncation_tie_goes_against_the_column_part() {
        let name = GeneratedName::foreign_key(
            "q23456789012345678901234567890",
            &strings(&["r23456789012345678901234567890"]),
        );
        assert_eq!(
            name.unsuffixed(),
            "q2345678901234567890123456789_r234567890123456789012345678_fkey"
        );
    }

    #[test]
    fn truncation_cuts_a_multi_column_list_as_one_string() {
        let elements = [
            column("a23456789012345678901234567890"),
            column("b23456789012345678901234567890"),
            column("c23456789012345678901234567890"),
        ];
        assert_eq!(
            GeneratedName::index("mc", &elements, &[]).unsuffixed(),
            "mc_a23456789012345678901234567890_b234567890123456789012345_idx"
        );
    }

    #[test]
    fn truncation_clips_a_multibyte_table_name_to_a_char_boundary() {
        let name = GeneratedName::primary_key("aååååååååååååååååååååååååååååååå").unsuffixed();
        assert_eq!(name, "aåååååååååååååååååååååååååååå_pkey");
        assert_eq!(name.len(), 62);
    }

    #[test]
    fn truncation_clips_a_multibyte_column_name_to_a_char_boundary() {
        let name =
            GeneratedName::unique("tm", &strings(&["bööööööööööööööööööööööööööööööö"]), &[])
                .unsuffixed();
        assert_eq!(name, "tm_bööööööööööööööööööööööööööö_key");
        assert_eq!(name.len(), 62);
    }

    const D63: &str = "d23456789012345678901234567890123456789012345678901234567890123";

    #[test]
    fn truncation_first_unique_on_63_char_table() {
        let name = GeneratedName::unique(D63, &strings(&["a"]), &[]);
        assert_eq!(
            name.unsuffixed(),
            "d23456789012345678901234567890123456789012345678901234567_a_key"
        );
    }

    #[test]
    fn truncation_collision_label_key1_cuts_the_table_one_byte_more() {
        assert_eq!(
            make_object_name(D63, Some("a"), "key1"),
            "d2345678901234567890123456789012345678901234567890123456_a_key1"
        );
    }

    #[test]
    fn truncation_first_index_on_63_char_table() {
        let name = GeneratedName::index(D63, &[column("a")], &[]);
        assert_eq!(
            name.unsuffixed(),
            "d23456789012345678901234567890123456789012345678901234567_a_idx"
        );
    }

    #[test]
    fn truncation_collision_label_idx1_cuts_the_table_one_byte_more() {
        assert_eq!(
            make_object_name(D63, Some("a"), "idx1"),
            "d2345678901234567890123456789012345678901234567890123456_a_idx1"
        );
    }

    #[test]
    fn truncation_tables_sharing_a_58_char_prefix_get_the_same_first_choice() {
        let aaa = GeneratedName::primary_key(
            "z23456789012345678901234567890123456789012345678901234567890aaa",
        );
        let bbb = GeneratedName::primary_key(
            "z23456789012345678901234567890123456789012345678901234567890bbb",
        );
        assert_eq!(
            aaa.unsuffixed(),
            "z234567890123456789012345678901234567890123456789012345678_pkey"
        );
        assert_eq!(aaa.unsuffixed(), bbb.unsuffixed());
    }

    #[test]
    fn truncation_collision_label_pkey1_cuts_the_table_one_byte_more() {
        assert_eq!(
            make_object_name(
                "z23456789012345678901234567890123456789012345678901234567890bbb",
                None,
                "pkey1"
            ),
            "z23456789012345678901234567890123456789012345678901234567_pkey1"
        );
    }
}
