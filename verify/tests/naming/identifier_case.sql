-- @claim: Generated names use the stored identifiers verbatim: "Orders" gives Orders_pkey, not orders_pkey
-- @claim: Unquoted identifiers are folded to lower case before the name is built
-- @claim: Quoted characters such as spaces carry into the generated name unchanged
-- @min_version: 14

CREATE SCHEMA app;
SET search_path = app, public;

CREATE TABLE "Orders" (id int PRIMARY KEY, "Code" text UNIQUE, "Qty" int CHECK ("Qty" > 0));
CREATE INDEX ON "Orders" ("Qty");
SELECT assert_eq(constraint_names('app."Orders"', 'p'), 'Orders_pkey', '"Orders" PK is Orders_pkey');
SELECT assert_eq(constraint_index('app."Orders"', 'Orders_pkey'), 'app.Orders_pkey', '"Orders" PK index is Orders_pkey');
SELECT assert_eq(constraint_names('app."Orders"', 'u'), 'Orders_Code_key', '"Code" UNIQUE is Orders_Code_key');
SELECT assert_eq(constraint_names('app."Orders"', 'c'), 'Orders_Qty_check', '"Qty" CHECK is Orders_Qty_check');
SELECT assert_eq(index_names('app."Orders"'), 'app.Orders_Code_key,app.Orders_Qty_idx,app.Orders_pkey',
    'CREATE INDEX on "Qty" is Orders_Qty_idx');

CREATE TABLE MixedCase (Id int PRIMARY KEY);
SELECT assert_eq(constraint_names('app.mixedcase', 'p'), 'mixedcase_pkey',
    'unquoted MixedCase folds to mixedcase_pkey');

CREATE TABLE "order lines" ("line no" int UNIQUE);
SELECT assert_eq(constraint_names('app."order lines"', 'u'), 'order lines_line no_key',
    'spaces in quoted identifiers carry into the name');
