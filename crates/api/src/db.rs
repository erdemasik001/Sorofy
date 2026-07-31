//! Verification result cache (PLAN Day2 item 3).
//!
//! One table, append-only in spirit: every verification job is a row, and a
//! lookup returns the newest row for a contract id or wasm hash. SQLite because
//! the deploy target is a single node and a single file is the whole ops story.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Lifecycle of a verification job as the API reports it.
///
/// `verifier_core::VerificationResult` covers the engine's outcomes; this adds
/// the queue states around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    Verified,
    Mismatch,
    /// The job ran and failed (build error, bad source, infra fault).
    Error,
}

impl JobStatus {
    fn as_str(self) -> &'static str {
        match self {
            JobStatus::Pending => "pending",
            JobStatus::Verified => "verified",
            JobStatus::Mismatch => "mismatch",
            JobStatus::Error => "error",
        }
    }

    fn parse(s: &str) -> anyhow::Result<Self> {
        Ok(match s {
            "pending" => JobStatus::Pending,
            "verified" => JobStatus::Verified,
            "mismatch" => JobStatus::Mismatch,
            "error" => JobStatus::Error,
            other => anyhow::bail!("unknown job status in db: {other}"),
        })
    }
}

/// A cached verification, as stored.
#[derive(Debug, Clone, Serialize)]
pub struct VerificationRow {
    pub id: i64,
    pub contract_id: Option<String>,
    /// The hash the job set out to reproduce (lowercase hex).
    pub wasm_hash: String,
    /// The `SourceRef` the job built, as JSON.
    pub source: serde_json::Value,
    pub bldimg: String,
    pub status: JobStatus,
    /// Full `ReproductionReport` for finished jobs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<serde_json::Value>,
    /// Failure description when `status == error`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Handle to the cache. Cheap to clone; one connection behind a mutex.
///
/// SQLite serializes writers anyway, and every operation here is a point
/// read/write — holding a mutex across them adds no real contention at MVP
/// scale (jobs are minutes long; queries are microseconds).
#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

/// Ordered schema migrations. **Append only, never edit or reorder** — a
/// deployed database records how many of these it has applied, and rewriting a
/// past entry would silently diverge old and new deployments.
///
/// The applied count lives in SQLite's own `user_version` pragma, so the
/// mechanism needs no bookkeeping table and no dependency. Migration 1 is the
/// original MVP schema, written with `IF NOT EXISTS` so a pre-migrations database
/// (which already has those objects at `user_version = 0`) adopts the versioning
/// without a rebuild.
const MIGRATIONS: &[&str] = &[
    // 1 — initial schema.
    "CREATE TABLE IF NOT EXISTS verifications (
         id          INTEGER PRIMARY KEY,
         contract_id TEXT,
         wasm_hash   TEXT NOT NULL,
         source      TEXT NOT NULL,
         bldimg      TEXT NOT NULL,
         status      TEXT NOT NULL,
         report      TEXT,
         error       TEXT,
         created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
         updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
     );
     CREATE INDEX IF NOT EXISTS idx_verifications_wasm_hash
         ON verifications(wasm_hash);
     CREATE INDEX IF NOT EXISTS idx_verifications_contract_id
         ON verifications(contract_id);",
];

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("opening sqlite db at {}", path.display()))?;
        Self::init(conn)
    }

    /// In-memory database, for tests.
    pub fn open_in_memory() -> anyhow::Result<Self> {
        Self::init(Connection::open_in_memory().context("opening in-memory sqlite db")?)
    }

    fn init(conn: Connection) -> anyhow::Result<Self> {
        migrate(&conn)?;
        Ok(Db(Arc::new(Mutex::new(conn))))
    }

    /// How many migrations this database has applied.
    pub fn schema_version(&self) -> anyhow::Result<u32> {
        read_user_version(&self.conn())
    }

    /// Write a consistent snapshot of the cache to `dest` (roadmap 0.5).
    ///
    /// `VACUUM INTO` is SQLite's online backup: it runs inside a read transaction,
    /// so the copy is a valid database even while jobs are writing — unlike
    /// copying the file, which can catch a torn write. The copy is also compacted.
    /// `dest` must not already exist; SQLite refuses to overwrite.
    pub fn backup_to(&self, dest: &Path) -> anyhow::Result<()> {
        let dest_str = dest
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("backup path is not valid UTF-8: {}", dest.display()))?;
        self.conn()
            .execute("VACUUM INTO ?1", params![dest_str])
            .with_context(|| format!("backing up cache to {}", dest.display()))?;
        Ok(())
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        // A poisoned mutex means a previous holder panicked mid-operation;
        // SQLite transactions make the data itself safe to keep using.
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Cheap liveness check: confirm the connection still answers a query.
    ///
    /// Backs `GET /health` — a locked or corrupted cache surfaces as a failed ping
    /// rather than only showing up when a real request tries to read or write.
    pub fn ping(&self) -> anyhow::Result<()> {
        self.conn()
            .query_row("SELECT 1", [], |_| Ok(()))
            .context("db ping")
    }

    /// Record a new job as pending; returns its row id.
    pub fn insert_pending(
        &self,
        contract_id: Option<&str>,
        wasm_hash: &str,
        source: &serde_json::Value,
        bldimg: &str,
    ) -> anyhow::Result<i64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO verifications (contract_id, wasm_hash, source, bldimg, status)
             VALUES (?1, ?2, ?3, ?4, 'pending')",
            params![
                contract_id,
                wasm_hash.to_lowercase(),
                source.to_string(),
                bldimg
            ],
        )
        .context("inserting pending verification")?;
        Ok(conn.last_insert_rowid())
    }

    /// Mark a finished job with its outcome and full report.
    pub fn complete(
        &self,
        id: i64,
        status: JobStatus,
        report: &serde_json::Value,
    ) -> anyhow::Result<()> {
        self.conn()
            .execute(
                "UPDATE verifications
                 SET status = ?2, report = ?3,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now')
                 WHERE id = ?1",
                params![id, status.as_str(), report.to_string()],
            )
            .context("recording verification result")?;
        Ok(())
    }

    /// Mark a job as failed with a human-readable reason.
    pub fn fail(&self, id: i64, error: &str) -> anyhow::Result<()> {
        self.conn()
            .execute(
                "UPDATE verifications
                 SET status = 'error', error = ?2,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now')
                 WHERE id = ?1",
                params![id, error],
            )
            .context("recording verification failure")?;
        Ok(())
    }

    /// Fetch one row by id.
    pub fn get(&self, id: i64) -> anyhow::Result<Option<VerificationRow>> {
        self.conn()
            .query_row(
                &format!("SELECT {COLUMNS} FROM verifications WHERE id = ?1"),
                params![id],
                row_from_sql,
            )
            .optional()
            .context("looking up verification by id")
    }

    /// Newest row whose `contract_id` or `wasm_hash` equals `key`.
    ///
    /// One lookup for both shapes: a `C…` strkey only ever matches contract
    /// ids, a hex hash only ever matches hashes, so a single query keeps the
    /// caller's GET endpoint trivially simple.
    pub fn lookup(&self, key: &str) -> anyhow::Result<Option<VerificationRow>> {
        self.conn()
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM verifications
                     WHERE contract_id = ?1 OR wasm_hash = ?2
                     ORDER BY id DESC LIMIT 1"
                ),
                params![key, key.to_lowercase()],
                row_from_sql,
            )
            .optional()
            .context("looking up verification by contract id / wasm hash")
    }
}

/// Apply any migrations this database has not seen yet.
///
/// Each pending migration runs in its own transaction together with the
/// `user_version` bump, so a crash mid-migration leaves the database at the last
/// fully-applied version rather than half-migrated.
fn migrate(conn: &Connection) -> anyhow::Result<()> {
    let applied = read_user_version(conn)?;
    let target = MIGRATIONS.len() as u32;

    // A database from a newer binary: its schema may have objects this build does
    // not know about, and running an older migration over it could corrupt data.
    // Refuse rather than guess — the operator should redeploy the newer build or
    // restore a backup.
    anyhow::ensure!(
        applied <= target,
        "database schema version {applied} is newer than this build understands \
         ({target}); run the newer build or restore a compatible backup"
    );

    for (idx, sql) in MIGRATIONS.iter().enumerate().skip(applied as usize) {
        let version = idx as u32 + 1;
        conn.execute_batch(&format!(
            "BEGIN; {sql} PRAGMA user_version = {version}; COMMIT;"
        ))
        .with_context(|| format!("applying schema migration {version}"))?;
        tracing::info!(version, "applied schema migration");
    }
    Ok(())
}

fn read_user_version(conn: &Connection) -> anyhow::Result<u32> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .context("reading schema version")
}

const COLUMNS: &str =
    "id, contract_id, wasm_hash, source, bldimg, status, report, error, created_at, updated_at";

fn row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<VerificationRow> {
    // rusqlite's closure must return its own error type; JSON/status parse
    // failures can only come from a corrupted row we wrote ourselves.
    let parse = |idx: usize, e: &dyn std::fmt::Display| {
        rusqlite::Error::FromSqlConversionFailure(
            idx,
            rusqlite::types::Type::Text,
            format!("{e}").into(),
        )
    };
    let source_raw: String = row.get(3)?;
    let status_raw: String = row.get(5)?;
    let report_raw: Option<String> = row.get(6)?;
    Ok(VerificationRow {
        id: row.get(0)?,
        contract_id: row.get(1)?,
        wasm_hash: row.get(2)?,
        source: serde_json::from_str(&source_raw).map_err(|e| parse(3, &e))?,
        bldimg: row.get(4)?,
        status: JobStatus::parse(&status_raw).map_err(|e| parse(5, &e))?,
        report: report_raw
            .map(|r| serde_json::from_str(&r).map_err(|e| parse(6, &e)))
            .transpose()?,
        error: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> serde_json::Value {
        serde_json::json!({"kind": "git", "repo": "https://example.com/x", "rev": "abc"})
    }

    #[test]
    fn pending_then_complete_roundtrips() {
        let db = Db::open_in_memory().unwrap();
        let id = db
            .insert_pending(Some("CABC"), "AABB01", &source(), "img@sha256:x")
            .unwrap();

        let row = db.get(id).unwrap().expect("row exists");
        assert_eq!(row.status, JobStatus::Pending);
        // Hashes are canonicalized to lowercase on the way in.
        assert_eq!(row.wasm_hash, "aabb01");

        db.complete(
            id,
            JobStatus::Verified,
            &serde_json::json!({"result": "verified"}),
        )
        .unwrap();
        let row = db.get(id).unwrap().expect("row exists");
        assert_eq!(row.status, JobStatus::Verified);
        assert!(row.report.is_some());
    }

    #[test]
    fn lookup_matches_contract_id_and_hash_and_prefers_newest() {
        let db = Db::open_in_memory().unwrap();
        let first = db
            .insert_pending(Some("CID1"), "aa11", &source(), "img")
            .unwrap();
        db.fail(first, "boom").unwrap();
        let second = db
            .insert_pending(Some("CID1"), "aa11", &source(), "img")
            .unwrap();

        // Same key via contract id and via (case-insensitive) hash.
        assert_eq!(db.lookup("CID1").unwrap().expect("found").id, second);
        assert_eq!(db.lookup("AA11").unwrap().expect("found").id, second);
        assert!(db.lookup("unknown").unwrap().is_none());
    }

    /// A throwaway directory for the on-disk tests; removed on drop.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos();
            let path = std::env::temp_dir()
                .join(format!("sorofy-db-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&path).expect("create temp dir");
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_fresh_db_is_migrated_to_the_current_version() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len() as u32);
    }

    #[test]
    fn reopening_migrates_once_and_keeps_data() {
        let dir = TempDir::new("reopen");
        let path = dir.0.join("cache.db");

        let db = Db::open(&path).unwrap();
        let id = db
            .insert_pending(Some("CABC"), "aabb", &source(), "img")
            .unwrap();
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len() as u32);
        drop(db);

        // Reopening is idempotent: no re-run, no error, rows survive.
        let again = Db::open(&path).unwrap();
        assert_eq!(again.schema_version().unwrap(), MIGRATIONS.len() as u32);
        assert_eq!(again.get(id).unwrap().expect("row survived").id, id);
    }

    #[test]
    fn a_pre_migrations_database_adopts_versioning_without_losing_rows() {
        let dir = TempDir::new("legacy");
        let path = dir.0.join("legacy.db");

        // Simulate a database written before migrations existed: the MVP schema is
        // present but `user_version` was never set.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.execute(
                "INSERT INTO verifications (contract_id, wasm_hash, source, bldimg, status)
                 VALUES ('COLD', 'dead', '{}', 'img', 'verified')",
                [],
            )
            .unwrap();
            assert_eq!(read_user_version(&conn).unwrap(), 0);
        }

        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len() as u32);
        // The pre-existing row is still there — migration 1 is CREATE IF NOT EXISTS.
        assert!(db.lookup("COLD").unwrap().is_some());
    }

    #[test]
    fn a_database_from_a_newer_build_is_refused() {
        let dir = TempDir::new("newer");
        let path = dir.0.join("future.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            // Pretend a future build applied more migrations than we know about.
            conn.execute_batch(&format!("PRAGMA user_version = {}", MIGRATIONS.len() + 5))
                .unwrap();
        }
        // `Db` is not Debug, so match rather than `expect_err`.
        let err = match Db::open(&path) {
            Ok(_) => panic!("a newer schema must be refused"),
            Err(e) => e,
        };
        assert!(
            format!("{err:#}").contains("newer than this build"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn backup_writes_a_readable_copy_of_the_data() {
        let dir = TempDir::new("backup");
        let db = Db::open(&dir.0.join("live.db")).unwrap();
        let id = db
            .insert_pending(Some("CBAK"), "beef", &source(), "img")
            .unwrap();

        let backup_path = dir.0.join("snapshot.db");
        db.backup_to(&backup_path).unwrap();
        assert!(backup_path.exists(), "backup file should exist");

        // The copy opens as a normal database, at the same schema version, with the
        // row intact — i.e. it is restorable, not just bytes on disk.
        let restored = Db::open(&backup_path).unwrap();
        assert_eq!(
            restored.schema_version().unwrap(),
            db.schema_version().unwrap()
        );
        assert_eq!(restored.get(id).unwrap().expect("row in backup").id, id);
    }

    #[test]
    fn backup_refuses_to_overwrite_an_existing_file() {
        let dir = TempDir::new("nooverwrite");
        let db = Db::open(&dir.0.join("live.db")).unwrap();
        let dest = dir.0.join("taken.db");
        std::fs::write(&dest, b"do not clobber me").unwrap();

        assert!(
            db.backup_to(&dest).is_err(),
            "an existing destination must not be overwritten"
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"do not clobber me");
    }
}
