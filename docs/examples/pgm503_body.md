Detects tables that have no primary key but have at least one UNIQUE constraint where all constituent columns are NOT NULL. This is functionally equivalent to a PK but less conventional.

**Example** (flagged):
```sql
CREATE TABLE users (
  email text NOT NULL UNIQUE,
  name text
);
```

**Fix**:
```sql
CREATE TABLE users (
  email text PRIMARY KEY,
  name text
);
```

When PGM503 fires, [PGM502](#pgm502) does not fire for the same table.
