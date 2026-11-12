-- @claim: An unnamed FOREIGN KEY is named {table}_{col}_fkey, inline REFERENCES or table-level
-- @claim: A multi-column FK is {table}_{c1}_{c2}_fkey, named after the referencing columns in FK order
-- @claim: The referenced table and columns never appear in the generated name
-- @claim: ALTER TABLE ADD FOREIGN KEY and ADD COLUMN ... REFERENCES follow the same rule
-- @claim: An FK creates no index; conindid points at the referenced table's unique index
-- @min_version: 14

CREATE SCHEMA app;
SET search_path = app, public;

CREATE TABLE parent (id int PRIMARY KEY, x int, UNIQUE (id, x));

CREATE TABLE fk_inline (parent_id int REFERENCES parent);
SELECT assert_eq(constraint_names('app.fk_inline', 'f'), 'fk_inline_parent_id_fkey',
    'inline REFERENCES is {table}_{col}_fkey');

CREATE TABLE fk_table_level (pid int, FOREIGN KEY (pid) REFERENCES parent (id));
SELECT assert_eq(constraint_names('app.fk_table_level', 'f'), 'fk_table_level_pid_fkey',
    'table-level FOREIGN KEY is {table}_{col}_fkey, named after the referencing column');

CREATE TABLE fk_multi (px int, pid int, FOREIGN KEY (pid, px) REFERENCES parent (id, x));
SELECT assert_eq(constraint_names('app.fk_multi', 'f'), 'fk_multi_pid_px_fkey',
    'multi-column FK is {table}_{c1}_{c2}_fkey in FK column order');

CREATE TABLE fk_alter (pid int);
ALTER TABLE fk_alter ADD FOREIGN KEY (pid) REFERENCES parent;
SELECT assert_eq(constraint_names('app.fk_alter', 'f'), 'fk_alter_pid_fkey',
    'ALTER TABLE ADD FOREIGN KEY is {table}_{col}_fkey');

CREATE TABLE fk_add_column (x int);
ALTER TABLE fk_add_column ADD COLUMN pid int REFERENCES parent;
SELECT assert_eq(constraint_names('app.fk_add_column', 'f'), 'fk_add_column_pid_fkey',
    'ALTER TABLE ADD COLUMN ... REFERENCES is {table}_{col}_fkey');

SELECT assert_eq(index_names('app.fk_inline'), '<none>',
    'an FK creates no index on the referencing table');
SELECT assert_eq(constraint_index('app.fk_inline', 'fk_inline_parent_id_fkey'), 'app.parent_pkey',
    'FK conindid is the referenced table''s PK index');
