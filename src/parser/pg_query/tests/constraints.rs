use rstest::rstest;

use crate::{
    IrNode,
    parser::{AlterTableAction, IndexColumn, QualifiedName, TableConstraint, pg_query::parse_sql},
};

/// Every table constraint a statement declares, from CREATE TABLE (inline and
/// table-level) or from ALTER TABLE ADD CONSTRAINT / ADD COLUMN.
fn parsed_constraints(sql: &str) -> Vec<TableConstraint<QualifiedName>> {
    parse_sql(sql)
        .into_iter()
        .flat_map(|located| match located.node {
            IrNode::CreateTable(ct) => ct.constraints,
            IrNode::AlterTable(at) => at
                .actions
                .into_iter()
                .filter_map(|action| match action {
                    AlterTableAction::AddConstraint { constraint, .. } => Some(constraint),
                    _ => None,
                })
                .collect(),
            other => panic!("Expected CreateTable or AlterTable for `{sql}`, got: {other:?}"),
        })
        .collect()
}

fn check_referenced_columns(sql: &str) -> Vec<Vec<String>> {
    parsed_constraints(sql)
        .into_iter()
        .filter_map(|constraint| match constraint {
            TableConstraint::Check {
                referenced_columns, ..
            } => Some(referenced_columns),
            _ => None,
        })
        .collect()
}

#[rstest]
#[case::column_level("CREATE TABLE ck_column (a int CHECK (a > 0));", &["a"])]
#[case::table_level_one_column("CREATE TABLE ck_table_one (a int, b int, CHECK (b > 0));", &["b"])]
#[case::table_level_two_columns("CREATE TABLE ck_table_two (a int, b int, CHECK (a < b));", &["a", "b"])]
#[case::table_level_no_column("CREATE TABLE ck_table_none (a int, CHECK (true));", &[])]
#[case::column_level_referencing_another_column(
    "CREATE TABLE ck_column_other (a int, b int CHECK (b > a));",
    &["b", "a"]
)]
#[case::column_level_referencing_only_another_column(
    "CREATE TABLE ck_column_foreign (a int, b int CHECK (a > 0));",
    &["a"]
)]
#[case::column_level_no_column("CREATE TABLE ck_column_none (a int CHECK (true));", &[])]
#[case::repeated_column_counted_once(
    "CREATE TABLE ck_repeat (a int, CHECK (a + a > 0 AND a < 100));",
    &["a"]
)]
#[case::column_inside_function_call_and_cast(
    "CREATE TABLE ck_function (a int, CHECK (length(a::text) > 0));",
    &["a"]
)]
#[case::between_bounds("CREATE TABLE ck_between (a int, b int, c int, CHECK (a BETWEEN b AND c));", &["a", "b", "c"])]
#[case::in_list("CREATE TABLE ck_in_list (a int, CHECK (0 IN (a, 1)));", &["a"])]
#[case::boolean_test("CREATE TABLE ck_boolean_test (flag bool, CHECK (flag IS NOT FALSE));", &["flag"])]
#[case::subscript("CREATE TABLE ck_subscript (arr int[], CHECK (arr[1] > 0));", &["arr"])]
#[case::is_json("CREATE TABLE ck_is_json (doc text, CHECK (doc IS JSON OBJECT));", &["doc"])]
#[case::is_json_two_columns(
    "CREATE TABLE ck_is_json_two (doc text, alt text, CHECK (doc IS JSON OR alt IS JSON));",
    &["doc", "alt"]
)]
#[case::alter_table_add_check("ALTER TABLE ck_alter ADD CHECK (a < b);", &["a", "b"])]
#[case::alter_table_add_column_check(
    "ALTER TABLE ck_add_column ADD COLUMN c int CHECK (c > a);",
    &["c", "a"]
)]
fn test_check_referenced_columns(#[case] sql: &str, #[case] expected: &[&str]) {
    assert_eq!(check_referenced_columns(sql), [expected]);
}

fn include_columns(constraint: &TableConstraint<QualifiedName>) -> &[String] {
    match constraint {
        TableConstraint::PrimaryKey {
            include_columns, ..
        }
        | TableConstraint::Unique {
            include_columns, ..
        }
        | TableConstraint::Exclude {
            include_columns, ..
        } => include_columns,
        other => panic!("Expected a constraint that takes INCLUDE, got: {other:?}"),
    }
}

#[rstest]
#[case::unique("CREATE TABLE uq_include (a int, b int, UNIQUE (a) INCLUDE (b));")]
#[case::primary_key("CREATE TABLE pk_include (a int, b int, PRIMARY KEY (a) INCLUDE (b));")]
#[case::exclude(
    "CREATE TABLE ex_include (a int, b int, EXCLUDE USING btree (a WITH =) INCLUDE (b));"
)]
#[case::alter_table_add_unique("ALTER TABLE uq_alter ADD UNIQUE (a) INCLUDE (b);")]
fn test_constraint_include_columns(#[case] sql: &str) {
    let constraints = parsed_constraints(sql);
    assert_eq!(constraints.len(), 1, "{constraints:?}");
    assert_eq!(include_columns(&constraints[0]), ["b"]);
}

#[rstest]
#[case::inline_unique("CREATE TABLE uq_inline (a int UNIQUE);")]
#[case::inline_primary_key("CREATE TABLE pk_inline (a int PRIMARY KEY);")]
#[case::table_level_unique("CREATE TABLE uq_multi (a int, b int, UNIQUE (a, b));")]
#[case::exclude("CREATE TABLE ex_btree (a int, EXCLUDE USING btree (a WITH =));")]
fn test_constraint_without_include_has_no_include_columns(#[case] sql: &str) {
    let constraints = parsed_constraints(sql);
    assert_eq!(constraints.len(), 1, "{constraints:?}");
    assert!(include_columns(&constraints[0]).is_empty());
}

fn exclude_generated_name_parts(sql: &str) -> Vec<String> {
    parsed_constraints(sql)
        .iter()
        .flat_map(|constraint| match constraint {
            TableConstraint::Exclude { elements, .. } => elements
                .iter()
                .map(|element| element.generated_name_part().to_owned())
                .collect::<Vec<_>>(),
            other => panic!("Expected Exclude, got: {other:?}"),
        })
        .collect()
}

#[rstest]
#[case::columns(
    "CREATE TABLE ex_multi (a int, b int, EXCLUDE USING btree (a WITH =, b WITH =));",
    &["a", "b"]
)]
#[case::function_call(
    "CREATE TABLE ex_function (e text, EXCLUDE USING btree (lower(e) WITH =));",
    &["lower"]
)]
#[case::operator(
    "CREATE TABLE ex_operator (a int, EXCLUDE USING btree ((a + 1) WITH =));",
    &["expr"]
)]
fn test_exclude_element_generated_name_parts(#[case] sql: &str, #[case] expected: &[&str]) {
    assert_eq!(exclude_generated_name_parts(sql), expected);
}

#[test]
fn test_exclude_parenthesized_column_element_is_stored_as_the_column() {
    let constraints =
        parsed_constraints("CREATE TABLE ex_paren (a int, EXCLUDE USING btree (((a)) WITH =));");
    match &constraints[..] {
        [TableConstraint::Exclude { elements, .. }] => {
            assert_eq!(elements, &[IndexColumn::Column("a".to_string())])
        }
        other => panic!("Expected one Exclude, got: {other:?}"),
    }
}

#[test]
fn test_exclude_collated_expression_element_records_the_column_under_the_collation() {
    let constraints = parsed_constraints(
        "CREATE TABLE ex_collate (e text, EXCLUDE USING btree ((lower(e) COLLATE \"C\") WITH =));",
    );
    match &constraints[..] {
        [TableConstraint::Exclude { elements, .. }] => match &elements[..] {
            [
                IndexColumn::Expression {
                    referenced_columns, ..
                },
            ] => assert_eq!(referenced_columns, &["e"]),
            other => panic!("Expected one Expression element, got: {other:?}"),
        },
        other => panic!("Expected one Exclude, got: {other:?}"),
    }
}

#[test]
fn test_exclude_expression_element_stores_its_generated_name_part() {
    let constraints = parsed_constraints(
        "CREATE TABLE ex_function (e text, EXCLUDE USING btree (lower(e) WITH =));",
    );
    match &constraints[..] {
        [TableConstraint::Exclude { elements, .. }] => assert_eq!(
            elements,
            &[IndexColumn::Expression {
                text: "lower(e)".to_string(),
                referenced_columns: vec!["e".to_string()],
                generated_name_part: "lower".to_string(),
            }]
        ),
        other => panic!("Expected one Exclude, got: {other:?}"),
    }
}

fn parsed_create_table(sql: &str) -> crate::parser::CreateTable<QualifiedName> {
    match parse_sql(sql)
        .into_iter()
        .map(|located| located.node)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [IrNode::CreateTable(ct)] => ct.clone(),
        other => panic!("Expected one CreateTable for `{sql}`, got: {other:?}"),
    }
}

fn nullability_by_column(ct: &crate::parser::CreateTable<QualifiedName>) -> Vec<(&str, bool)> {
    ct.columns
        .iter()
        .map(|column| (column.name.as_str(), column.nullable))
        .collect()
}

#[rstest]
#[case::after_the_column("CREATE TABLE nn_after (a int, b int, NOT NULL a);")]
#[case::before_the_column("CREATE TABLE nn_before (NOT NULL a, a int, b int);")]
#[case::named("CREATE TABLE nn_named (a int, b int, CONSTRAINT nn_named_a_nn NOT NULL a);")]
#[case::not_valid("CREATE TABLE nn_not_valid (a int, b int, NOT NULL a NOT VALID);")]
#[case::no_inherit("CREATE TABLE nn_no_inherit (a int, b int, NOT NULL a NO INHERIT);")]
fn test_table_level_not_null_makes_its_column_not_null(#[case] sql: &str) {
    let ct = parsed_create_table(sql);
    assert_eq!(nullability_by_column(&ct), [("a", false), ("b", true)]);
    assert!(ct.constraints.is_empty(), "{:?}", ct.constraints);
}

#[test]
fn test_table_level_not_null_on_a_column_the_statement_does_not_list_changes_no_column() {
    let ct = parsed_create_table(
        "CREATE TABLE nn_partition PARTITION OF nn_parent (NOT NULL a) FOR VALUES IN (1);",
    );
    assert!(ct.columns.is_empty(), "{:?}", ct.columns);
    assert!(ct.constraints.is_empty(), "{:?}", ct.constraints);
}

fn parsed_alter_actions(sql: &str) -> Vec<AlterTableAction<QualifiedName>> {
    match parse_sql(sql)
        .into_iter()
        .map(|located| located.node)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [IrNode::AlterTable(at)] => at.actions.clone(),
        other => panic!("Expected one AlterTable for `{sql}`, got: {other:?}"),
    }
}

fn set_not_null(column_name: &str) -> AlterTableAction<QualifiedName> {
    AlterTableAction::SetNotNull {
        column_name: column_name.to_string(),
    }
}

#[rstest]
#[case::unnamed("ALTER TABLE nn_alter ADD NOT NULL a;", &["a"])]
#[case::named("ALTER TABLE nn_alter ADD CONSTRAINT nn_alter_a_nn NOT NULL a;", &["a"])]
#[case::no_inherit("ALTER TABLE nn_alter ADD NOT NULL a NO INHERIT;", &["a"])]
#[case::two_columns("ALTER TABLE nn_alter ADD NOT NULL a, ADD NOT NULL b;", &["a", "b"])]
fn test_alter_table_add_not_null_is_set_not_null(#[case] sql: &str, #[case] columns: &[&str]) {
    let expected: Vec<_> = columns.iter().map(|column| set_not_null(column)).collect();
    assert_eq!(parsed_alter_actions(sql), expected);
}

#[test]
fn test_alter_table_add_not_null_not_valid_changes_no_column() {
    assert_eq!(
        parsed_alter_actions(
            "ALTER TABLE nn_alter ADD CONSTRAINT nn_alter_a_nn NOT NULL a NOT VALID;"
        ),
        [AlterTableAction::Other {
            description: "ADD NOT NULL a NOT VALID".to_string(),
        }]
    );
}

fn not_valid_flags(sql: &str) -> Vec<bool> {
    parsed_constraints(sql)
        .into_iter()
        .map(|constraint| match constraint {
            TableConstraint::Check { not_valid, .. }
            | TableConstraint::ForeignKey { not_valid, .. } => not_valid,
            other => panic!("Expected CHECK or FOREIGN KEY for `{sql}`, got: {other:?}"),
        })
        .collect()
}

#[rstest]
#[case::column_check("CREATE TABLE ne_check (a int CHECK (a > 0) NOT ENFORCED);", &[true])]
#[case::column_foreign_key(
    "ALTER TABLE ne_fk ADD COLUMN c bigint REFERENCES ne_parent NOT ENFORCED;",
    &[true]
)]
#[case::attribute_in_between(
    "ALTER TABLE ne_fk ADD COLUMN c bigint REFERENCES ne_parent DEFERRABLE NOT ENFORCED;",
    &[true]
)]
#[case::only_the_constraint_before_it(
    "CREATE TABLE ne_two (a int CHECK (a > 0) NOT ENFORCED CHECK (a < 10));",
    &[true, false]
)]
#[case::column_enforced(
    "ALTER TABLE ne_fk ADD COLUMN c bigint REFERENCES ne_parent ENFORCED;",
    &[false]
)]
#[case::table_check("ALTER TABLE ne_table ADD CHECK (a > 0) NOT ENFORCED;", &[true])]
#[case::table_foreign_key(
    "ALTER TABLE ne_table ADD FOREIGN KEY (a) REFERENCES ne_parent NOT ENFORCED;",
    &[true]
)]
fn test_not_enforced_reads_as_not_valid(#[case] sql: &str, #[case] expected: &[bool]) {
    assert_eq!(not_valid_flags(sql), expected);
}
