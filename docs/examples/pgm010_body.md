Detects `ALTER TABLE ... DROP COLUMN` where the dropped column participates in a UNIQUE constraint or unique index. PostgreSQL automatically drops dependent constraints, silently removing uniqueness guarantees.

**Example** (bad):
```sql
-- Table has UNIQUE(email)
ALTER TABLE users DROP COLUMN email;
-- The unique constraint is silently removed.
```

**Fix**: Verify that the uniqueness guarantee is no longer needed before dropping the column.

See also [PGM011](#pgm011), [PGM012](#pgm012).
