-- @claim: ALTER TABLE ... RENAME TO keeps every generated constraint and index name (orders_pkey stays orders_pkey)
-- @claim: ALTER TABLE ... RENAME COLUMN keeps generated names that contain the old column name
-- @claim: The old names stay taken, so a new table reusing the old table name gets suffixed names (orders_pkey1)
-- @claim: Renaming an index-backed constraint renames its index, and renaming the index renames the constraint
-- @min_version: 14

CREATE SCHEMA app;
SET search_path = app, public;

CREATE TABLE customers (id int PRIMARY KEY);
CREATE TABLE orders (
    id int PRIMARY KEY,
    code text UNIQUE,
    qty int CHECK (qty > 0),
    customer_id int REFERENCES customers,
    slot int,
    EXCLUDE USING btree (slot WITH =));
CREATE INDEX ON orders (qty);

ALTER TABLE orders RENAME TO purchases;
SELECT assert_eq(constraint_names('app.purchases', 'p'), 'orders_pkey', 'RENAME TO keeps orders_pkey');
SELECT assert_eq(constraint_index('app.purchases', 'orders_pkey'), 'app.orders_pkey', 'RENAME TO keeps the PK index name');
SELECT assert_eq(constraint_names('app.purchases', 'u'), 'orders_code_key', 'RENAME TO keeps the UNIQUE name');
SELECT assert_eq(constraint_names('app.purchases', 'c'), 'orders_qty_check', 'RENAME TO keeps the CHECK name');
SELECT assert_eq(constraint_names('app.purchases', 'f'), 'orders_customer_id_fkey', 'RENAME TO keeps the FK name');
SELECT assert_eq(constraint_names('app.purchases', 'x'), 'orders_slot_excl', 'RENAME TO keeps the EXCLUDE name');
SELECT assert_eq(index_names('app.purchases'),
    'app.orders_code_key,app.orders_pkey,app.orders_qty_idx,app.orders_slot_excl',
    'RENAME TO keeps every index name, including the CREATE INDEX one');

ALTER TABLE purchases RENAME COLUMN code TO sku;
ALTER TABLE purchases RENAME COLUMN qty TO quantity;
ALTER TABLE purchases RENAME COLUMN customer_id TO buyer_id;
ALTER TABLE purchases RENAME COLUMN slot TO position;
SELECT assert_eq(
    constraint_names('app.purchases', 'u') || ',' || constraint_names('app.purchases', 'c') || ','
        || constraint_names('app.purchases', 'f') || ',' || constraint_names('app.purchases', 'x'),
    'orders_code_key,orders_qty_check,orders_customer_id_fkey,orders_slot_excl',
    'RENAME COLUMN keeps constraint names that contain the old column name');
SELECT assert_eq(index_names('app.purchases'),
    'app.orders_code_key,app.orders_pkey,app.orders_qty_idx,app.orders_slot_excl',
    'RENAME COLUMN keeps index names that contain the old column name');

CREATE TABLE orders (id int PRIMARY KEY, code text UNIQUE, qty int CHECK (qty > 0), customer_id int REFERENCES customers);
CREATE INDEX ON orders (qty);
SELECT assert_eq(constraint_names('app.orders', 'p'), 'orders_pkey1',
    'new table reusing a renamed table''s name: PK is orders_pkey1');
SELECT assert_eq(constraint_names('app.orders', 'u'), 'orders_code_key1', 'new table: UNIQUE is orders_code_key1');
SELECT assert_eq(constraint_names('app.orders', 'c'), 'orders_qty_check1', 'new table: CHECK is orders_qty_check1');
SELECT assert_eq(constraint_names('app.orders', 'f'), 'orders_customer_id_fkey1', 'new table: FK is orders_customer_id_fkey1');
SELECT assert_eq(index_names('app.orders'), 'app.orders_code_key1,app.orders_pkey1,app.orders_qty_idx1',
    'new table: every index name gets suffix 1');

CREATE TABLE rename_con (a int UNIQUE);
ALTER TABLE rename_con RENAME CONSTRAINT rename_con_a_key TO rename_con_new_key;
SELECT assert_eq(index_names('app.rename_con'), 'app.rename_con_new_key',
    'RENAME CONSTRAINT on a UNIQUE renames its index too');

CREATE TABLE rename_idx (id int PRIMARY KEY);
ALTER INDEX rename_idx_pkey RENAME TO rename_idx_new_pk;
SELECT assert_eq(constraint_names('app.rename_idx', 'p'), 'rename_idx_new_pk',
    'ALTER INDEX ... RENAME on a PK index renames the constraint too');
