Detects `CREATE TABLE IF NOT EXISTS` targeting a table that already exists in the migration history. `IF NOT EXISTS` makes the statement a silent no-op — if the column definitions differ from the actual table state, the migration author may believe the table has the shape described in this statement, when in reality PostgreSQL ignores it entirely.

**Example** (bad):
```sql
-- V001: original table
CREATE TABLE orders (id bigint PRIMARY KEY);
ALTER TABLE orders ADD COLUMN status text NOT NULL DEFAULT 'pending';

-- V010: redundant re-creation (silently ignored)
CREATE TABLE IF NOT EXISTS orders (
  id bigint PRIMARY KEY,
  status text NOT NULL DEFAULT 'pending',
  created_at timestamptz DEFAULT now()  -- this column will NOT be added
);
```

**Fix**: Remove the redundant `CREATE TABLE IF NOT EXISTS`. If the intent is to add columns, use `ALTER TABLE ... ADD COLUMN` instead.
