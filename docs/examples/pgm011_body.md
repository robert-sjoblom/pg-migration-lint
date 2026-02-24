Detects `ALTER TABLE ... DROP COLUMN` where the dropped column participates in the table's primary key. The table loses its row identity, affecting replication, ORMs, query planning, and data integrity.

**Example** (bad):
```sql
-- Table has PRIMARY KEY (id)
ALTER TABLE orders DROP COLUMN id;
-- The primary key is silently removed.
```

**Fix**: Add a new primary key on remaining columns before or after dropping the column.

See also [PGM010](#pgm010), [PGM012](#pgm012).
