Detects columns declared as `timestamp` (which PostgreSQL interprets as `timestamp without time zone`). This type stores no timezone context, making values ambiguous across environments.

**Example** (bad):
```sql
CREATE TABLE events (created_at timestamp NOT NULL);
```

**Fix**:
```sql
CREATE TABLE events (created_at timestamptz NOT NULL);
```
