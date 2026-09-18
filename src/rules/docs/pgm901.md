Not a standalone lint rule. When a migration file is identified as a down migration, all findings from other rules are capped to INFO severity. Down migrations are informational only — they represent the undo path and are not expected to follow the same safety rules as forward migrations.

Detection is by filename suffix: the stem (filename minus `.sql` extension) must end with `.down` or `_down`. Examples:
- `000001_create_users.down.sql` — detected (go-migrate convention)
- `V001__create_users_down.sql` — detected (underscore convention)
- `downtown_orders.sql` — **not** detected (contains "down" but not as a suffix)

Liquibase `<rollback>` blocks are not currently detected as down migrations.

This rule cannot be suppressed (it is applied automatically by the pipeline).
