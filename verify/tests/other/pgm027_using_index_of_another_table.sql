-- @claim: ADD PRIMARY KEY / ADD UNIQUE ... USING INDEX naming another table's index in the altered table's schema fails with 55000 "index ... does not belong to table ..." (PGM027)
-- @claim: When that index already backs a constraint of its own table, the statement fails first with 55000 "index ... is already associated with a constraint"
-- @claim: The index is looked up in the altered table's schema only; a same-named index in another schema reads as missing (42704)
-- @claim: The failed statement changes neither table
-- @min_version: 14

CREATE SCHEMA app;
CREATE SCHEMA other;
SET search_path = app, public;

CREATE FUNCTION statement_error(stmt text) RETURNS text
LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE stmt;
    RETURN '<no error>';
EXCEPTION WHEN OTHERS THEN
    RETURN SQLSTATE || ' ' || SQLERRM;
END;
$$;

CREATE TABLE customers (id int NOT NULL, email text NOT NULL, CONSTRAINT customers_pkey PRIMARY KEY (id));
CREATE UNIQUE INDEX customers_email_idx ON customers (email);
CREATE TABLE orders (id int NOT NULL, email text NOT NULL);
CREATE TABLE other.remote (email text NOT NULL);
CREATE UNIQUE INDEX remote_email_idx ON other.remote (email);

SELECT assert_eq(statement_error('ALTER TABLE orders ADD PRIMARY KEY USING INDEX customers_email_idx'),
    '55000 index "customers_email_idx" does not belong to table "orders"',
    'ADD PRIMARY KEY USING INDEX of another table''s index fails');
SELECT assert_eq(statement_error('ALTER TABLE orders ADD CONSTRAINT orders_email_key UNIQUE USING INDEX customers_email_idx'),
    '55000 index "customers_email_idx" does not belong to table "orders"',
    'ADD CONSTRAINT ... UNIQUE USING INDEX of another table''s index fails');
SELECT assert_eq(statement_error('ALTER TABLE orders ADD UNIQUE USING INDEX customers_pkey'),
    '55000 index "customers_pkey" is already associated with a constraint',
    'USING INDEX of another table''s constraint index fails on the constraint first');
SELECT assert_eq(statement_error('ALTER TABLE orders ADD UNIQUE USING INDEX remote_email_idx'),
    '42704 index "remote_email_idx" does not exist',
    'USING INDEX does not look in another schema');

SELECT assert_eq(constraint_names('app.orders', 'p') || ' ' || constraint_names('app.orders', 'u') || ' ' || index_names('app.orders'),
    '<none> <none> <none>',
    'the failed statements leave the altered table without a constraint or index');
SELECT assert_eq(constraint_names('app.customers', 'p') || ' ' || index_names('app.customers'),
    'customers_pkey app.customers_email_idx,app.customers_pkey',
    'the failed statements leave the index on its own table, unrenamed');
