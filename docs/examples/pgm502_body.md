Detects `CREATE TABLE` (non-temporary) that results in a table without a primary key after the entire file is processed.

**Example** (bad):
```sql
CREATE TABLE events (event_type text, payload jsonb);
```

**Fix**:
```sql
CREATE TABLE events (
  id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  event_type text,
  payload jsonb
);
```

Temporary tables are excluded. When [PGM503](#pgm503) fires (UNIQUE NOT NULL substitute detected), PGM502 does not fire for the same table.
