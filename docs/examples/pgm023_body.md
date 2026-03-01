Detects `VACUUM FULL` targeting an existing table. `VACUUM FULL` rewrites the entire table into a new data file under an ACCESS EXCLUSIVE lock, blocking all reads and writes for the duration. On large tables this means minutes to hours of downtime.

**Example** (flagged):
```sql
VACUUM FULL orders;
```

**Fix** (use an online compaction tool):
```
pg_repack --table orders --no-superuser-check -d mydb
```

Or schedule during a maintenance window when downtime is acceptable.
