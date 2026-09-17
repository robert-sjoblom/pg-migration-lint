Detects `DROP SCHEMA ... CASCADE`. This is the most destructive single DDL statement in PostgreSQL — it silently drops every object in the schema: tables, views, sequences, functions, types, and indexes.

Unlike `DROP TABLE CASCADE` (which only removes objects that depend on one table), `DROP SCHEMA CASCADE` destroys the entire namespace and everything in it.

**Example**:
```sql
DROP SCHEMA myschema CASCADE;
-- Silently drops every table, view, function, sequence, and type in 'myschema'.
```

**Recommended approach**:
1. Enumerate all objects in the schema before dropping.
2. Explicitly drop or migrate each object in separate migration steps.
3. Use plain `DROP SCHEMA` (without `CASCADE`) so PostgreSQL will error if the schema is non-empty.
