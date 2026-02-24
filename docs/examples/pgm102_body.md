Detects timestamp columns with precision 0. Precision 0 causes **rounding**, not truncation — a value of `'23:59:59.9'` rounds to the next day.

**Example** (bad):
```sql
CREATE TABLE events (created_at timestamptz(0));
```

**Fix**:
```sql
CREATE TABLE events (created_at timestamptz);
```
