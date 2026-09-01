# PostgreSQL behaviour verification

`pg-migration-lint` makes factual claims about PostgreSQL in its rule explanations — that
`DROP INDEX` blocks reads, that `char(n)` pads, that `ATTACH PARTITION` only needs SHARE
UPDATE EXCLUSIVE. This directory checks those claims against real servers, PostgreSQL 14
through 18, so a claim cannot quietly become folklore.

Nothing here is part of the analyzer. `pg-migration-lint` has no database dependency and
never connects to one.

## The two suites

| | `tests/*.sql` | `lock-tests/` |
|---|---|---|
| Runs | one session | two or more concurrent sessions |
| Written in | SQL + plpgsql assertions | Rust (`postgres`, sync client) |
| Driven by | `run.sh` → `psql` in the container | `cargo test` |
| Good for | type behaviour, rewrites, catalog state, index propagation, plan shapes | lock modes, blocking, lock queues, transaction-scoped locks, SQLSTATEs |

**Which one does my test go in?** If it needs more than one session, or it cares about
timing or about *who* got blocked, it goes in `lock-tests/`. Otherwise SQL is less
ceremony. Lock behaviour is inherently multi-session, so in practice every lock claim now
lives in `lock-tests/` — the SQL suite's old `dblink` lock probes were replaced by real
connections, which can name the SQLSTATE a blocked statement received instead of just
observing that "something was raised".

## Running

Both suites need the servers up:

```bash
cd verify && docker compose up -d --wait
```

PG 14–18 land on ports 54314–54318. No local `psql` or Rust database setup is needed; the
SQL suite runs `psql` inside the container, and the Rust suite connects over the mapped
ports.

```bash
# SQL suite — all tests, all versions, prints a pass/fail matrix
./run.sh
./run.sh --pg 17                  # one version
./run.sh --test types/pgm101      # matching tests only
./run.sh --verbose --no-teardown  # per-assertion output, leave containers running

# Rust suite
cd lock-tests
cargo test
cargo test --test partition_drop_blocked_by_traffic   # one file
cargo test reader_on_parent                           # matching test names
```

A version whose container is unreachable is **skipped** by the Rust suite, with a note on
stderr. Set `PG_LOCK_TESTS_REQUIRE_ALL=1` to make that a failure instead — CI does, so a
container that never started cannot pass as a green run.

## Adding a test

**SQL suite.** Drop a `.sql` file under the matching `tests/` subdirectory. Start with
`-- @claim:` lines saying what it establishes, and `-- @min_version: N` if it does not apply
to every version. Call `assert_true` / `assert_false` / `assert_eq` /
`assert_explain_contains` from `lib/framework.sql`; the runner collects the results table.
Each file gets a freshly created database.

**Rust suite.** Add a file under `lock-tests/tests/`. One `#[rstest]` function per claim,
parameterized `#[values(14, 15, 16, 17, 18)]`, so a failure is attributable to one claim on
one version. Each test opens its own database via `TestDb::new(pg, "<unique_label>")` —
the label becomes the database name and **must be unique across the crate**. See
`lock-tests/src/lib.rs` for the harness API, and
`tests/partition_drop_blocked_by_traffic.rs` for the house style.

## CI

`.github/workflows/verify-pg.yml` runs both suites weekly and on manual dispatch — not per
PR, since it needs five database servers. `ci.yml` does lint `lock-tests` (fmt + clippy) on
every PR, because a test suite that no longer compiles is worse than no suite.

## Also here

Two generators build Rust source from a live server's catalogs, both against PG 18:

- `gen-reserved-keywords.sh` → `src/rules/reserved_keywords.rs`, from `pg_get_keywords()`
- `gen-fn-volatility.sh` → `src/rules/fn_volatility.rs`, from `pg_proc.provolatile`

`ci.yml` runs both with `--check` on every PR, so the committed tables cannot drift from
what PostgreSQL actually reports.
