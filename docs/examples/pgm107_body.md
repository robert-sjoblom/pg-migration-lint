Detects primary key columns that use `integer` (`int4`) or `smallint` (`int2`) instead of `bigint` (`int8`). High-write tables routinely exhaust the ~2.1 billion (`integer`) or ~32 000 (`smallint`) limit. Migrating to `bigint` later requires an ACCESS EXCLUSIVE lock and full table rewrite.

**Example** (flagged):
```sql
CREATE TABLE orders (id integer PRIMARY KEY);
```

**Fix**:
```sql
CREATE TABLE orders (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY
);
```
