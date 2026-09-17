Detects `UPDATE` targeting a table that already exists in the database. On large tables, updates hold row locks for the full statement duration, generate WAL (spiking replication lag), and may time out under migration tool limits.

**Example** (flagged):
```sql
UPDATE orders SET status = 'pending' WHERE status IS NULL;
```

**Recommended approach**:
1. Verify the row count is bounded (small lookup table = fine).
2. For large tables, batch the update in chunks.
3. Consider running the update outside the migration transaction.
