//! Verification result cache (PLAN Day2 item 3).
//!
//! Append-only in spirit: every verification job is a row in `verifications`, and
//! a lookup returns the newest row for a contract id or wasm hash. SQLite because
//! the deploy target is a single node and a single file is the whole ops story.
//!
//! `attestations` (hackathon STEP 5) is the outbox for the on-chain half: one row
//! per verification we intend to attest, carrying its own status, attempt count
//! and retry schedule. It is a *separate table* rather than columns on
//! `verifications` for two reasons — a retry needs state of its own, and
//! [`Db::fail_orphaned_pending`] sweeps `verifications` at startup, so an
//! attestation queued before a restart must not be caught by it. It is retried
//! instead (docs/hackathon-design.md §9).

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

/// Lifecycle of one queued attestation.
///
/// Three states and no fourth, because each has to be true of the ledger as well as of this
/// table: either we have not landed a transaction yet, or we have one and can name it, or the
/// registry refused in a way that will not change on a retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestationStatus {
    /// Queued, or waiting out a backoff after a failure that may yet clear.
    Pending,
    /// A transaction landed on-chain; `tx_hash` names it.
    Submitted,
    /// The registry refused permanently, or we gave up retrying. `last_error` says which.
    Rejected,
}

impl AttestationStatus {
    fn as_str(self) -> &'static str {
        match self {
            AttestationStatus::Pending => "pending",
            AttestationStatus::Submitted => "submitted",
            AttestationStatus::Rejected => "rejected",
        }
    }

    fn parse(s: &str) -> anyhow::Result<Self> {
        Ok(match s {
            "pending" => AttestationStatus::Pending,
            "submitted" => AttestationStatus::Submitted,
            "rejected" => AttestationStatus::Rejected,
            other => anyhow::bail!("unknown attestation status in db: {other}"),
        })
    }
}

/// One queued attestation, as stored.
///
/// The claim is held here in full rather than recomputed from the verification's report, so a
/// retry after a restart sends exactly the bytes the first attempt would have.
#[derive(Debug, Clone, Serialize)]
pub struct AttestationRow {
    pub id: i64,
    pub verification_id: i64,
    /// The claim, all three lowercase 32-byte hex digests.
    pub wasm_hash: String,
    pub input_digest: String,
    pub rebuilt_hash: String,
    pub status: AttestationStatus,
    /// The attest transaction, once one has landed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,
    /// How many times submission has been attempted.
    pub attempts: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Hard ceiling on rows one [`Db::recent`] call may return.
///
/// Generous for an explorer page, small enough that no single request can pull
/// the whole table into memory however the caller asks.
pub const MAX_PAGE: u32 = 200;

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
    // 2 — the attestation outbox (hackathon STEP 5). Rows are written only when
    // the attestation path is switched on; with it off the table simply stays
    // empty, and nothing else in the service reads or writes it.
    //
    // `verification_id` is UNIQUE so enqueuing is idempotent: one verification
    // makes at most one attestation, however many times the path is asked to
    // queue it. The REFERENCES clause documents the link; SQLite only enforces
    // it with `PRAGMA foreign_keys=ON`, which this database does not set.
    "CREATE TABLE IF NOT EXISTS attestations (
         id              INTEGER PRIMARY KEY,
         verification_id INTEGER NOT NULL UNIQUE REFERENCES verifications(id),
         wasm_hash       TEXT NOT NULL,
         input_digest    TEXT NOT NULL,
         rebuilt_hash    TEXT NOT NULL,
         status          TEXT NOT NULL,
         tx_hash         TEXT,
         attempts        INTEGER NOT NULL DEFAULT 0,
         last_error      TEXT,
         next_attempt_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
         created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
         updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
     );
     CREATE INDEX IF NOT EXISTS idx_attestations_due
         ON attestations(status, next_attempt_at);",
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

    /// Fail every job still marked `pending`, returning how many rows changed.
    ///
    /// Jobs live in the process, not the database: `POST /verify` records a
    /// pending row and spawns a task. So a row still `pending` when the service
    /// starts belongs to a process that no longer exists — its build container
    /// died with it, and nothing will ever finish the row. Left alone, `GET`
    /// reports `pending` forever for work that is not running, which is a lie the
    /// cache tells about its own state; the deploy model makes this routine
    /// rather than exotic, since every redeploy is a restart.
    ///
    /// Sound because the deploy model is a single node with a single writer
    /// (docs/security.md, S2): at startup this process owns no jobs, so every
    /// pending row is by definition an orphan. A second concurrent instance
    /// against the same file would break that assumption and fail live jobs —
    /// which is why this runs once at startup, not on a timer.
    pub fn fail_orphaned_pending(&self, error: &str) -> anyhow::Result<usize> {
        self.conn()
            .execute(
                "UPDATE verifications
                 SET status = 'error', error = ?1,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now')
                 WHERE status = 'pending'",
                params![error],
            )
            .context("reconciling orphaned pending jobs")
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

    /// Newest-first page of verifications, for the explorer's landing view.
    ///
    /// Ordered by `id` rather than `created_at`: the timestamp has one-second
    /// resolution, so two jobs submitted in the same second would come back in an
    /// arbitrary order and could repeat or vanish across pages. The primary key
    /// is monotonic and unique, which makes paging stable.
    ///
    /// `limit` is clamped to [`MAX_PAGE`] here rather than in the caller. The
    /// table grows without bound and this backs a public, unauthenticated read,
    /// so the ceiling belongs where it cannot be forgotten — a handler may still
    /// apply a smaller default, but it cannot lift this one.
    pub fn recent(&self, limit: u32, offset: u32) -> anyhow::Result<Vec<VerificationRow>> {
        let limit = limit.min(MAX_PAGE);
        let conn = self.conn();
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {COLUMNS} FROM verifications
                 ORDER BY id DESC LIMIT ?1 OFFSET ?2"
            ))
            .context("preparing recent verifications query")?;
        let rows = stmt
            .query_map(params![limit, offset], row_from_sql)
            .context("listing recent verifications")?
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("reading recent verifications")?;
        Ok(rows)
    }

    /// Total verifications on record, so a client can page without walking off
    /// the end.
    pub fn count(&self) -> anyhow::Result<i64> {
        self.conn()
            .query_row("SELECT COUNT(*) FROM verifications", [], |row| row.get(0))
            .context("counting verifications")
    }

    // ---- the attestation outbox (hackathon STEP 5) ---------------------------------------

    /// Queue an attestation for `verification_id`, due immediately.
    ///
    /// Returns the new row id, or `None` if this verification was already queued — which is
    /// what makes the call safe to repeat. Hashes are stored lowercase, like everywhere else.
    pub fn enqueue_attestation(
        &self,
        verification_id: i64,
        wasm_hash: &str,
        input_digest: &str,
        rebuilt_hash: &str,
    ) -> anyhow::Result<Option<i64>> {
        let conn = self.conn();
        let inserted = conn
            .execute(
                "INSERT OR IGNORE INTO attestations
                     (verification_id, wasm_hash, input_digest, rebuilt_hash, status)
                 VALUES (?1, ?2, ?3, ?4, 'pending')",
                params![
                    verification_id,
                    wasm_hash.to_lowercase(),
                    input_digest.to_lowercase(),
                    rebuilt_hash.to_lowercase(),
                ],
            )
            .context("queueing attestation")?;
        Ok((inserted > 0).then(|| conn.last_insert_rowid()))
    }

    /// The oldest attestation whose retry time has arrived, if any.
    ///
    /// Ordered by id so a backlog drains in the order it was verified.
    pub fn due_attestation(&self) -> anyhow::Result<Option<AttestationRow>> {
        self.conn()
            .query_row(
                &format!(
                    "SELECT {ATTESTATION_COLUMNS} FROM attestations
                     WHERE status = 'pending'
                       AND next_attempt_at <= strftime('%Y-%m-%dT%H:%M:%SZ','now')
                     ORDER BY id LIMIT 1"
                ),
                [],
                attestation_from_sql,
            )
            .optional()
            .context("looking up the next due attestation")
    }

    /// Seconds until the earliest pending attestation is due, or `None` if none is pending.
    ///
    /// Negative when one is already overdue; the caller decides what to do with that. Computed
    /// in SQLite so the comparison uses the same clock that wrote `next_attempt_at`.
    pub fn seconds_until_next_attestation(&self) -> anyhow::Result<Option<i64>> {
        self.conn()
            .query_row(
                "SELECT MIN(strftime('%s', next_attempt_at)) - strftime('%s','now')
                 FROM attestations WHERE status = 'pending'",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .context("looking up when the next attestation is due")
    }

    /// Record that an attest transaction landed on-chain.
    ///
    /// `tx_hash` is `None` when the transaction went through but the CLI named no hash we
    /// could read: the column stays NULL rather than holding a placeholder that would read
    /// like a real transaction.
    pub fn attestation_submitted(&self, id: i64, tx_hash: Option<&str>) -> anyhow::Result<()> {
        self.finish_attestation(id, AttestationStatus::Submitted, tx_hash, None)
    }

    /// Record a refusal that retrying cannot fix, or a give-up after too many attempts.
    pub fn attestation_rejected(&self, id: i64, reason: &str) -> anyhow::Result<()> {
        self.finish_attestation(id, AttestationStatus::Rejected, None, Some(reason))
    }

    fn finish_attestation(
        &self,
        id: i64,
        status: AttestationStatus,
        tx_hash: Option<&str>,
        error: Option<&str>,
    ) -> anyhow::Result<()> {
        self.conn()
            .execute(
                "UPDATE attestations
                 SET status = ?2, tx_hash = ?3, last_error = ?4,
                     attempts = attempts + 1,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now')
                 WHERE id = ?1",
                params![id, status.as_str(), tx_hash, error],
            )
            .context("recording attestation outcome")?;
        Ok(())
    }

    /// Record a failure that may yet clear, and schedule the next attempt `delay` from now.
    ///
    /// Returns the attempt count after the bump, so the caller can decide when to give up.
    pub fn attestation_retry_later(
        &self,
        id: i64,
        error: &str,
        delay: std::time::Duration,
    ) -> anyhow::Result<i64> {
        let conn = self.conn();
        conn.execute(
            "UPDATE attestations
             SET last_error = ?2,
                 attempts = attempts + 1,
                 next_attempt_at =
                     strftime('%Y-%m-%dT%H:%M:%SZ','now', ?3),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now')
             WHERE id = ?1",
            params![id, error, format!("+{} seconds", delay.as_secs())],
        )
        .context("rescheduling attestation")?;
        conn.query_row(
            "SELECT attempts FROM attestations WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .context("reading attestation attempt count")
    }

    /// The attestation queued for a verification, if there is one.
    pub fn attestation_for(&self, verification_id: i64) -> anyhow::Result<Option<AttestationRow>> {
        self.conn()
            .query_row(
                &format!(
                    "SELECT {ATTESTATION_COLUMNS} FROM attestations WHERE verification_id = ?1"
                ),
                params![verification_id],
                attestation_from_sql,
            )
            .optional()
            .context("looking up a verification's attestation")
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

const ATTESTATION_COLUMNS: &str = "id, verification_id, wasm_hash, input_digest, rebuilt_hash, \
     status, tx_hash, attempts, last_error, created_at, updated_at";

fn attestation_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttestationRow> {
    let status_raw: String = row.get(5)?;
    Ok(AttestationRow {
        id: row.get(0)?,
        verification_id: row.get(1)?,
        wasm_hash: row.get(2)?,
        input_digest: row.get(3)?,
        rebuilt_hash: row.get(4)?,
        status: AttestationStatus::parse(&status_raw).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                format!("{e}").into(),
            )
        })?,
        tx_hash: row.get(6)?,
        attempts: row.get(7)?,
        last_error: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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

    #[test]
    fn orphaned_pending_jobs_are_failed_and_finished_ones_untouched() {
        let db = Db::open_in_memory().unwrap();
        let orphan = db
            .insert_pending(Some("CORP"), "aa01", &source(), "img")
            .unwrap();
        let done = db
            .insert_pending(Some("CDON"), "aa02", &source(), "img")
            .unwrap();
        db.complete(
            done,
            JobStatus::Verified,
            &serde_json::json!({"result": "verified"}),
        )
        .unwrap();

        assert_eq!(db.fail_orphaned_pending("restarted").unwrap(), 1);

        let row = db.get(orphan).unwrap().expect("orphan row");
        assert_eq!(row.status, JobStatus::Error);
        assert_eq!(row.error.as_deref(), Some("restarted"));
        // A job that already reached a terminal state keeps its outcome.
        let row = db.get(done).unwrap().expect("finished row");
        assert_eq!(row.status, JobStatus::Verified);

        // Idempotent: a second startup finds nothing left to reconcile.
        assert_eq!(db.fail_orphaned_pending("restarted").unwrap(), 0);
    }

    #[test]
    fn recent_pages_newest_first_and_counts() {
        let db = Db::open_in_memory().unwrap();
        let mut ids = Vec::new();
        for n in 0..5 {
            ids.push(
                db.insert_pending(Some(&format!("C{n}")), &format!("aa0{n}"), &source(), "img")
                    .unwrap(),
            );
        }
        assert_eq!(db.count().unwrap(), 5);

        // Newest first, and the page boundary neither repeats nor skips a row —
        // the reason ordering is by id and not by the one-second `created_at`.
        let page1: Vec<i64> = db.recent(2, 0).unwrap().iter().map(|r| r.id).collect();
        let page2: Vec<i64> = db.recent(2, 2).unwrap().iter().map(|r| r.id).collect();
        assert_eq!(page1, [ids[4], ids[3]]);
        assert_eq!(page2, [ids[2], ids[1]]);

        // Past the end is empty, not an error.
        assert!(db.recent(10, 99).unwrap().is_empty());
    }

    #[test]
    fn recent_caps_the_page_however_much_is_asked_for() {
        let db = Db::open_in_memory().unwrap();
        for n in 0..=MAX_PAGE {
            db.insert_pending(None, &format!("{n:04x}"), &source(), "img")
                .unwrap();
        }
        assert_eq!(db.count().unwrap(), i64::from(MAX_PAGE) + 1);

        // The ceiling lives in the query, not in a caller's discipline: asking
        // for everything still yields one page.
        assert_eq!(db.recent(u32::MAX, 0).unwrap().len(), MAX_PAGE as usize);
    }

    // ---- the attestation outbox ----------------------------------------------------------

    /// A finished verification with an attestation queued against it.
    fn queued(db: &Db) -> (i64, i64) {
        let id = db
            .insert_pending(Some("CQUE"), &"aa".repeat(32), &source(), "img")
            .unwrap();
        db.complete(
            id,
            JobStatus::Verified,
            &serde_json::json!({"result": "verified"}),
        )
        .unwrap();
        let attestation = db
            .enqueue_attestation(id, &"AA".repeat(32), &"BB".repeat(32), &"CC".repeat(32))
            .unwrap()
            .expect("queued");
        (id, attestation)
    }

    #[test]
    fn queueing_an_attestation_stores_the_claim_and_is_idempotent() {
        let db = Db::open_in_memory().unwrap();
        let (id, attestation) = queued(&db);

        let row = db.attestation_for(id).unwrap().expect("row exists");
        assert_eq!(row.id, attestation);
        assert_eq!(row.status, AttestationStatus::Pending);
        assert_eq!(row.attempts, 0);
        assert!(row.tx_hash.is_none() && row.last_error.is_none());
        // Digests are canonicalized on the way in, like every other hash here.
        assert_eq!(row.wasm_hash, "aa".repeat(32));
        assert_eq!(row.input_digest, "bb".repeat(32));
        assert_eq!(row.rebuilt_hash, "cc".repeat(32));

        // A second attempt to queue the same verification adds nothing — which is what makes
        // the call safe to repeat after a crash, a retry, or a re-verification.
        assert_eq!(
            db.enqueue_attestation(id, &"11".repeat(32), &"22".repeat(32), &"33".repeat(32))
                .unwrap(),
            None
        );
        assert_eq!(
            db.attestation_for(id).unwrap().unwrap().wasm_hash,
            "aa".repeat(32)
        );
    }

    /// The reason attestations live in their own table at all.
    #[test]
    fn the_startup_sweep_fails_orphaned_jobs_and_leaves_the_outbox_alone() {
        let db = Db::open_in_memory().unwrap();
        let (id, _) = queued(&db);
        // A job that really was orphaned, so the sweep has something to do.
        let orphan = db
            .insert_pending(None, &"dd".repeat(32), &source(), "img")
            .unwrap();

        assert_eq!(db.fail_orphaned_pending("restarted").unwrap(), 1);
        assert_eq!(db.get(orphan).unwrap().unwrap().status, JobStatus::Error);

        // The queued attestation is still pending and still due: a restart retries it rather
        // than failing it (docs/hackathon-design.md §9).
        let row = db
            .attestation_for(id)
            .unwrap()
            .expect("outbox row survives");
        assert_eq!(row.status, AttestationStatus::Pending);
        assert_eq!(db.due_attestation().unwrap().map(|r| r.id), Some(row.id));
    }

    #[test]
    fn a_submitted_attestation_records_its_transaction_and_leaves_the_queue() {
        let db = Db::open_in_memory().unwrap();
        let (id, attestation) = queued(&db);

        let tx = "b7c50c102cfb435711015c3b23e5e80c18864297ab60befb480d9b0b78fdf85b";
        db.attestation_submitted(attestation, Some(tx)).unwrap();

        let row = db.attestation_for(id).unwrap().unwrap();
        assert_eq!(row.status, AttestationStatus::Submitted);
        assert_eq!(row.tx_hash.as_deref(), Some(tx));
        assert_eq!(row.attempts, 1, "the successful attempt counts");
        assert!(db.due_attestation().unwrap().is_none());
        assert_eq!(db.seconds_until_next_attestation().unwrap(), None);
    }

    #[test]
    fn a_transaction_we_could_not_name_is_left_null_rather_than_faked() {
        let db = Db::open_in_memory().unwrap();
        let (id, attestation) = queued(&db);
        db.attestation_submitted(attestation, None).unwrap();

        let row = db.attestation_for(id).unwrap().unwrap();
        assert_eq!(row.status, AttestationStatus::Submitted);
        assert_eq!(row.tx_hash, None);
    }

    #[test]
    fn a_rejected_attestation_keeps_the_reason_and_is_never_retried() {
        let db = Db::open_in_memory().unwrap();
        let (id, attestation) = queued(&db);
        db.attestation_rejected(attestation, "registry refused: … already attested … (#4)")
            .unwrap();

        let row = db.attestation_for(id).unwrap().unwrap();
        assert_eq!(row.status, AttestationStatus::Rejected);
        assert!(row.last_error.as_deref().unwrap().contains("#4"));
        assert!(db.due_attestation().unwrap().is_none());
    }

    #[test]
    fn a_rescheduled_attestation_is_not_due_until_its_delay_has_passed() {
        let db = Db::open_in_memory().unwrap();
        let (id, attestation) = queued(&db);
        assert!(
            db.due_attestation().unwrap().is_some(),
            "due as soon as it is queued"
        );

        let attempts = db
            .attestation_retry_later(attestation, "connection closed", Duration::from_secs(60))
            .unwrap();
        assert_eq!(attempts, 1);

        assert!(db.due_attestation().unwrap().is_none(), "not due yet");
        // The worker sleeps on this rather than polling; it must point at the delay just set.
        let wait = db
            .seconds_until_next_attestation()
            .unwrap()
            .expect("still pending");
        assert!((1..=60).contains(&wait), "unexpected wait: {wait}s");

        let row = db.attestation_for(id).unwrap().unwrap();
        assert_eq!(
            row.status,
            AttestationStatus::Pending,
            "a retry is still pending"
        );
        assert_eq!(row.last_error.as_deref(), Some("connection closed"));

        // Attempts accumulate across retries, which is what bounds them.
        assert_eq!(
            db.attestation_retry_later(attestation, "again", Duration::from_secs(60))
                .unwrap(),
            2
        );
    }

    #[test]
    fn a_backlog_drains_in_the_order_it_was_verified() {
        let db = Db::open_in_memory().unwrap();
        let mut queued_ids = Vec::new();
        for n in 0..3 {
            let id = db
                .insert_pending(None, &format!("{n:064x}"), &source(), "img")
                .unwrap();
            queued_ids.push(
                db.enqueue_attestation(
                    id,
                    &format!("{n:064x}"),
                    &"bb".repeat(32),
                    &"cc".repeat(32),
                )
                .unwrap()
                .unwrap(),
            );
        }
        // Oldest first, and each one leaves the queue as it reaches a terminal state.
        for expected in queued_ids {
            let due = db.due_attestation().unwrap().expect("one is due");
            assert_eq!(due.id, expected);
            db.attestation_submitted(due.id, Some(&"ee".repeat(32)))
                .unwrap();
        }
        assert!(db.due_attestation().unwrap().is_none());
    }

    #[test]
    fn an_existing_database_gains_the_outbox_without_losing_its_verifications() {
        let dir = TempDir::new("outbox-migration");
        let path = dir.0.join("v1.db");

        // A database from the build before STEP 5: migration 1 applied, and nothing else.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(&format!(
                "BEGIN; {} PRAGMA user_version = 1; COMMIT;",
                MIGRATIONS[0]
            ))
            .unwrap();
            conn.execute(
                "INSERT INTO verifications (contract_id, wasm_hash, source, bldimg, status)
                 VALUES ('COLD', 'dead', '{}', 'img', 'verified')",
                [],
            )
            .unwrap();
        }

        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len() as u32);
        assert!(
            db.lookup("COLD").unwrap().is_some(),
            "the old row is untouched"
        );
        // The new table is there and empty — an upgraded instance attests nothing it did
        // before it was told to.
        assert!(db.due_attestation().unwrap().is_none());
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
