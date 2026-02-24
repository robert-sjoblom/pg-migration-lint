Detects `CREATE TABLE` with the `UNLOGGED` keyword. Unlogged tables skip the write-ahead log for better write performance, but data is **truncated after a crash** and the table is **not replicated** to standby servers.

**Example** (flagged):
```sql
CREATE UNLOGGED TABLE scratch_data (id int, payload text);
```

**When unlogged tables are appropriate**:
- Ephemeral staging/import data that can be re-derived.
- Materialized caches where the source of truth lives elsewhere.
- ETL scratch space within a batch job.
