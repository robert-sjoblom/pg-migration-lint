Detects columns declared as `json`. The `json` type stores exact input text and re-parses on every operation. `jsonb` stores a decomposed binary format that is faster, smaller, indexable (GIN), and supports containment operators.

**Example** (bad):
```sql
CREATE TABLE events (payload json NOT NULL);
```

**Fix**:
```sql
CREATE TABLE events (payload jsonb NOT NULL);
```
