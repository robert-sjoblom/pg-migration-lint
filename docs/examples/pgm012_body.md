Detects `ALTER TABLE ... DROP COLUMN` where the dropped column participates in a FOREIGN KEY constraint. The referential integrity guarantee is silently lost, potentially allowing orphaned rows.

**Example** (bad):
```sql
-- Table has FOREIGN KEY (customer_id) REFERENCES customers(id)
ALTER TABLE orders DROP COLUMN customer_id;
-- The foreign key constraint is silently removed.
```

**Fix**: Verify that the referential integrity guarantee is no longer needed before dropping the column.

See also [PGM010](#pgm010), [PGM011](#pgm011).
