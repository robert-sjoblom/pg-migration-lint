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
//! Lock behaviour is multi-session: one session holds a lock, another discovers
//!  it cannot proceed. The harness models that with real connections, which
//! means an assertion can name *which* session was blocked and *which* SQLSTATE it
//! got; here the previous plpgsql/dblink harness could only observe "the probe
//! raised something".

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use postgres::{Client, Config, NoTls};

/// Re-exported so tests can name the SQLSTATEs they expect.
pub use postgres::error::SqlState;

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

/// How often to re-check whether a spawned statement is waiting on a lock.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How long to wait for a spawned statement to become observably lock-blocked.
const ENQUEUE_TIMEOUT: Duration = Duration::from_secs(10);

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
        Session::connect(&self.conn())
    }

    /// Everything needed to open a connection later, on another thread.
    fn conn(&self) -> ConnInfo {
        ConnInfo {
            port: self.port,
            name: self.name.clone(),
            version: self.version,
        }
    }
}

/// Connection details, detached from the `TestDb` borrow so a background thread can
/// open its own session.
#[derive(Clone)]
struct ConnInfo {
    port: u16,
    name: String,
    version: u32,
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
    fn connect(info: &ConnInfo) -> Session {
        let client = config(info.port, &info.name)
            .connect(NoTls)
            .unwrap_or_else(|e| {
                panic!(
                    "could not connect to {} on pg{}: {}",
                    info.name,
                    info.version,
                    detail(&e)
                )
            });
        Session {
            client,
            what: info.name.clone(),
        }
    }

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

    /// How many relation locks are currently *waiting* in this database.
    ///
    /// Restricted to `locktype = 'relation'` and to the current database, so it counts
    /// exactly the kind of wait a blocked DDL statement produces and nothing from a
    /// test running concurrently against another database.
    pub fn ungranted_relation_locks(&mut self) -> i64 {
        self.scalar_i64(
            "SELECT count(*)::bigint FROM pg_locks
              WHERE NOT granted
                AND locktype = 'relation'
                AND database = (SELECT oid FROM pg_database
                                 WHERE datname = current_database())",
        )
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

/// A session parked in an open transaction, holding whatever locks its statements
/// took. This is how we fake "live traffic."
///
/// Locks are released when the `Holder` is dropped.
pub struct Holder {
    session: Session,
}

impl Holder {
    /// The holding session, for running further statements in the same transaction.
    pub fn session(&mut self) -> &mut Session {
        &mut self.session
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.session.try_run("ROLLBACK");
    }
}

/// A statement running on its own thread because it is expected to block.
///
/// Dropping a `Waiter` deliberately does **not** join the thread: if the lock it
/// wants is still held, joining would hang the test. The thread instead dies when
/// `TestDb`'s `Drop` force-drops the database and terminates its connection.
pub struct Waiter {
    sql: String,
    handle: Option<std::thread::JoinHandle<Result<(), postgres::Error>>>,
}

impl Waiter {
    /// Blocks until the statement finishes, returning its result.
    pub fn join(mut self) -> Result<(), postgres::Error> {
        let handle = self.handle.take().expect("waiter already joined");
        handle
            .join()
            .unwrap_or_else(|_| panic!("waiter thread panicked running `{}`", oneline(&self.sql)))
    }

    /// The SQL this waiter is running.
    pub fn sql(&self) -> &str {
        &self.sql
    }
}

impl TestDb {
    /// Opens a session, starts a transaction, and runs `sql` inside it.
    ///
    /// Whatever locks `sql` acquires stay held until the returned [`Holder`] drops.
    pub fn hold(&self, sql: &str) -> Holder {
        let mut session = self.session();
        session.run("BEGIN");
        session.run(sql);
        Holder { session }
    }

    /// Lock modes currently granted on `relation`, in this database only.
    ///
    /// `pg_locks` is cluster-wide, tests run concurrently against the same
    /// server, and they reuse table names: an unscoped query would report
    /// another test's locks as this one's.
    pub fn locks_on(&self, relation: &str) -> Vec<String> {
        let mut s = self.session();
        let rows = s
            .client
            .query(
                "SELECT l.mode
                   FROM pg_locks l
                   JOIN pg_class c ON c.oid = l.relation
                  WHERE l.locktype = 'relation'
                    AND l.granted
                    AND l.database = (SELECT oid FROM pg_database
                                       WHERE datname = current_database())
                    AND c.relname = $1
                  ORDER BY l.mode",
                &[&relation],
            )
            .unwrap_or_else(|e| panic!("could not read pg_locks: {}", detail(&e)));
        rows.iter().map(|r| r.get::<_, String>(0)).collect()
    }

    /// Whether `mode` is currently granted on `relation` in this database.
    pub fn holds_lock(&self, relation: &str, mode: &str) -> bool {
        self.locks_on(relation).iter().any(|m| m == mode)
    }

    /// Runs `sql` on a background session, returning once it is observably waiting
    /// for a lock.
    ///
    /// Panics if the statement completes, fails, or never enters the lock queue — each
    /// of which would mean a later assertion about "what happens while this is
    /// blocked" is testing nothing.
    pub fn spawn_blocked(&self, sql: &str) -> Waiter {
        let info = self.conn();
        let owned = sql.to_string();
        let handle = std::thread::spawn(move || {
            let mut session = Session::connect(&info);
            session.try_run(&owned)
        });

        let mut observer = self.session();
        let deadline = Instant::now() + ENQUEUE_TIMEOUT;
        let mut gave_up = None;

        loop {
            if observer.ungranted_relation_locks() > 0 {
                break;
            }
            if handle.is_finished() {
                gave_up = Some("it finished instead of blocking");
                break;
            }
            if Instant::now() >= deadline {
                gave_up = Some("it never appeared in the lock queue");
                break;
            }
            std::thread::sleep(POLL_INTERVAL);
        }

        if let Some(why) = gave_up {
            if handle.is_finished() {
                let outcome = handle.join().unwrap_or_else(|_| {
                    panic!("waiter thread panicked running `{}`", oneline(sql))
                });
                panic!(
                    "expected `{}` to block on a lock, but {why}: {}",
                    oneline(sql),
                    match outcome {
                        Ok(()) => "it succeeded".to_string(),
                        Err(e) => detail(&e),
                    }
                );
            }
            panic!(
                "expected `{}` to block on a lock, but {why} (its thread is still running)",
                oneline(sql)
            );
        }

        Waiter {
            sql: sql.to_string(),
            handle: Some(handle),
        }
    }
}

/// Runs `sql` expecting it to fail with SQLSTATE `expected`, returning the error.
///
/// Checking the specific state is the point: "it errored" and "it could not get the
/// lock" are different claims, and only the second is about locking.
pub fn expect_sqlstate(
    session: &mut Session,
    sql: &str,
    expected: &SqlState,
    claim: &str,
) -> postgres::Error {
    match session.try_run(sql) {
        Ok(()) => panic!(
            "{claim}\n  expected `{}` to fail with SQLSTATE {}, but it succeeded",
            oneline(sql),
            expected.code()
        ),
        Err(e) if e.code() == Some(expected) => e,
        Err(e) => panic!(
            "{claim}\n  expected `{}` to fail with SQLSTATE {}, but it failed with {}",
            oneline(sql),
            expected.code(),
            detail(&e)
        ),
    }
}

/// Runs `sql` expecting it to give up waiting for a lock (SQLSTATE 55P03).
///
/// The session needs a `lock_timeout` set, or this waits forever.
pub fn expect_lock_timeout(session: &mut Session, sql: &str, claim: &str) -> postgres::Error {
    expect_sqlstate(session, sql, &SqlState::LOCK_NOT_AVAILABLE, claim)
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
