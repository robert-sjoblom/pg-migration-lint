Detects `INSERT INTO` targeting a table that already exists in the database (not created in the same migration file). While seed data and lookup table population are valid use cases, they deserve review because large inserts can cause lock contention, WAL pressure, and timeouts.

**Example** (flagged):
```sql
INSERT INTO config (key, value) VALUES ('feature_x', 'enabled');
```

Not flagged when inserting into a table created in the same migration file.
