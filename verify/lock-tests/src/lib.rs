//! Verification harness for PostgreSQL lock behaviour.
//!
//! These tests exist to check the factual claims made in `pg-migration-lint` rule
//! explanations against real PostgreSQL servers (14–18). They are deliberately a
//! separate package from the analyzer so that `pg-migration-lint` itself never gains
//! a Postgres client in its dependency graph.
//!
//! Servers come from `verify/docker-compose.yml`, which maps PG 14–18 to ports
//! 54314–54318 on the loopback interface:
//!
//! ```text
//! cd verify && docker compose up -d
//! cd verify/lock-tests && cargo test
//! ```
//!
//! Every test gets its own freshly created database; tests are independent and
//! run in parallel. A version whose container is unreachable is skipped with a note
//! on stderr, unless `PG_LOCK_TESTS_REQUIRE_ALL=1` is set, in which case an
//! unreachable server is a failure.
//!
//! # Why sessions, not dblink
//!
//! Lock behaviour is inherently multi-session: one session holds a lock, another
//! discovers it cannot proceed. The harness models that with real connections, which
//! means an assertion can name *which* session was blocked and *which* SQLSTATE it
//! got; here the previous plpgsql/dblink harness could only observe "the probe
//! raised something".

use std::fmt::Write as _;

use postgres::error::SqlState;
use postgres::{Client, Config, NoTls};

/// PG major versions paired with their port.
pub const PG_VERSIONS: [(u32, u16); 5] = [
    (14, 54314),
    (15, 54315),
    (16, 54316),
    (17, 54317),
    (18, 54318),
];

/// How long a probe waits for a lock before giving up.
///
/// The lock under test is held by an *open transaction* rather than by an
/// in-progress statement, so this value only bounds how long a blocked probe
/// waits.
const PROBE_LOCK_TIMEOUT: &str = "250ms";

/// Host port for a given major version, or `None` if the version isn't covered.
pub fn port_for(version: u32) -> Option<u16> {
    PG_VERSIONS
        .iter()
        .find(|(v, _)| *v == version)
        .map(|(_, p)| *p)
}

fn require_all() -> bool {
    std::env::var("PG_LOCK_TESTS_REQUIRE_ALL").is_ok_and(|v| v == "1")
}

fn config(port: u16, dbname: &str) -> Config {
    let mut cfg = Config::new();
    cfg.host("127.0.0.1")
        .port(port)
        .user("postgres")
        .password("test")
        .dbname(dbname);
    cfg
}

/// A freshly created, private database on one PostgreSQL server.
///
/// Dropped databases take their connections with them (`DROP DATABASE ... WITH
/// (FORCE)`). A test that panics mid-transaction still cleans up.
pub struct TestDb {
    version: u32,
    port: u16,
    name: String,
}

impl TestDb {
    /// Creates a fresh database for `label` on `version`.
    ///
    /// `label` must be unique across the whole crate. Convention is
    /// `"<rule_id>_<short_claim>"`, e.g. `"pgm013_blocks_select"`. A duplicate
    /// surfaces as a `CREATE DATABASE`/`DROP DATABASE` failure once both tests run.
    ///
    /// Returns `None` when that server is unreachable, so the caller can skip.
    /// With `PG_LOCK_TESTS_REQUIRE_ALL=1` an unreachable server panics instead.
    pub fn new(version: u32, label: &str) -> Option<Self> {
        let port =
            port_for(version).unwrap_or_else(|| panic!("no port mapped for PostgreSQL {version}"));

        // Database names are capped at 63 bytes; keep the tail.
        let mut name = format!("lt_{label}_{version}");
        name.retain(|c| c.is_ascii_alphanumeric() || c == '_');
        if name.len() > 63 {
            name = name.split_off(name.len() - 63);
        }

        let mut admin = match config(port, "postgres").connect(NoTls) {
            Ok(client) => client,
            Err(e) => {
                if require_all() {
                    panic!(
                        "PG {version} unreachable on port {port} and \
                         PG_LOCK_TESTS_REQUIRE_ALL=1: {e}"
                    );
                }
                eprintln!(
                    "SKIP pg{version}: unreachable on 127.0.0.1:{port} \
                     (run `cd verify && docker compose up -d`): {e}"
                );
                return None;
            }
        };

        // Without FORCE, a live collision fails ("database is being
        // accessed by other users") while a stale database from a killed run is
        // cleaned up.
        for stmt in [
            format!("DROP DATABASE IF EXISTS \"{name}\""),
            format!("CREATE DATABASE \"{name}\""),
        ] {
            admin.batch_execute(&stmt).unwrap_or_else(|e| {
                panic!(
                    "could not prepare database {name} on pg{version} via `{stmt}`: {}",
                    detail(&e)
                )
            });
        }

        Some(TestDb {
            version,
            port,
            name,
        })
    }

    /// The major version this database lives on.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// Opens an independent connection to this database.
    ///
    /// Each `Session` is its own backend, which is what makes lock conflicts
    /// observable.
    pub fn session(&self) -> Session {
        let client = config(self.port, &self.name)
            .connect(NoTls)
            .unwrap_or_else(|e| {
                panic!(
                    "could not connect to {} on pg{}: {}",
                    self.name,
                    self.version,
                    detail(&e)
                )
            });
        Session {
            client,
            what: self.name.clone(),
        }
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let Ok(mut admin) = config(self.port, "postgres").connect(NoTls) else {
            return;
        };
        // FORCE terminates any connection the test left behind, including one stuck
        // in an open transaction after a panic.
        let _ = admin.batch_execute(&format!(
            "DROP DATABASE IF EXISTS \"{}\" WITH (FORCE);",
            self.name
        ));
    }
}

/// One PostgreSQL backend connection.
pub struct Session {
    client: Client,
    what: String,
}

impl Session {
    /// Runs SQL, panicking on any error.
    pub fn run(&mut self, sql: &str) {
        if let Err(e) = self.client.batch_execute(sql) {
            panic!(
                "[{}] failed to run `{}`: {}",
                self.what,
                oneline(sql),
                detail(&e)
            );
        }
    }

    /// Runs SQL, returning the error instead of panicking.
    pub fn try_run(&mut self, sql: &str) -> Result<(), postgres::Error> {
        self.client.batch_execute(sql)
    }

    /// Runs a query expected to yield exactly one `bigint`.
    ///
    /// Cast in SQL (`count(*)::bigint`) — PostgreSQL's `int4` and `int8` are distinct
    /// types to the client.
    pub fn scalar_i64(&mut self, sql: &str) -> i64 {
        let row = self.client.query_one(sql, &[]).unwrap_or_else(|e| {
            panic!(
                "[{}] failed to query `{}`: {}",
                self.what,
                oneline(sql),
                detail(&e)
            )
        });
        row.get(0)
    }

    /// Runs a query expected to yield exactly one `bool`.
    pub fn scalar_bool(&mut self, sql: &str) -> bool {
        let row = self.client.query_one(sql, &[]).unwrap_or_else(|e| {
            panic!(
                "[{}] failed to query `{}`: {}",
                self.what,
                oneline(sql),
                detail(&e)
            )
        });
        row.get(0)
    }

    /// Sets this session's `lock_timeout`.
    pub fn set_lock_timeout(&mut self, value: &str) {
        self.run(&format!("SET lock_timeout = '{value}'"));
    }
}

/// Formats an error together with the server's own message.
///
/// `postgres::Error` renders as the bare string "db error" on its own.
pub fn detail(e: &postgres::Error) -> String {
    if let Some(db) = e.as_db_error() {
        let mut s = format!("{}: {}", db.code().code(), db.message());
        if let Some(d) = db.detail() {
            let _ = write!(s, " (detail: {d})");
        }
        if let Some(h) = db.hint() {
            let _ = write!(s, " (hint: {h})");
        }
        return s;
    }

    let mut s = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(inner) = source {
        let _ = write!(s, ": {inner}");
        source = inner.source();
    }
    s
}

/// Whether an error is specifically a lock timeout (SQLSTATE 55P03).
pub fn is_lock_timeout(e: &postgres::Error) -> bool {
    e.code() == Some(&SqlState::LOCK_NOT_AVAILABLE)
}

/// What happened to a probe that ran while `ddl` held its locks.
#[derive(Debug, PartialEq, Eq)]
enum Probe {
    Blocked,
    Allowed,
}

/// Runs `ddl` in an open transaction on one session, then attempts `probe_sql`
/// from a second session with a short `lock_timeout`.
///
/// Panics if the probe fails for any reason other than a lock timeout.
fn probe(db: &TestDb, ddl: &str, probe_sql: &str) -> Probe {
    let mut holder = db.session();
    holder.run("BEGIN");
    holder.run(ddl);

    let mut prober = db.session();
    prober.set_lock_timeout(PROBE_LOCK_TIMEOUT);

    let outcome = match prober.try_run(probe_sql) {
        Ok(()) => Probe::Allowed,
        Err(e) if is_lock_timeout(&e) => Probe::Blocked,
        Err(e) => panic!(
            "probe `{}` failed for a reason other than a lock timeout \
             (so this test proves nothing about locking): {}",
            oneline(probe_sql),
            detail(&e)
        ),
    };

    holder.run("ROLLBACK");
    outcome
}

/// Asserts that `probe_sql` cannot proceed while `ddl` holds its locks.
pub fn assert_lock_blocks(db: &TestDb, ddl: &str, probe_sql: &str, claim: &str) {
    if probe(db, ddl, probe_sql) == Probe::Allowed {
        panic!(
            "{claim}\n  pg{}: expected `{}` to be BLOCKED while `{}` held its lock, \
             but it completed",
            db.version(),
            oneline(probe_sql),
            oneline(ddl),
        );
    }
}

/// Asserts that `probe_sql` proceeds even while `ddl` holds its locks.
pub fn assert_lock_allows(db: &TestDb, ddl: &str, probe_sql: &str, claim: &str) {
    if probe(db, ddl, probe_sql) == Probe::Blocked {
        panic!(
            "{claim}\n  pg{}: expected `{}` to be ALLOWED while `{}` held its lock, \
             but it timed out waiting for a lock",
            db.version(),
            oneline(probe_sql),
            oneline(ddl),
        );
    }
}

/// Collapses SQL to a single line for readable panic messages.
fn oneline(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    for word in sql.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        let _ = write!(out, "{word}");
    }
    if out.len() > 160 {
        out.truncate(157);
        out.push_str("...");
    }
    out
}
