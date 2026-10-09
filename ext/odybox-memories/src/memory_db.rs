//! Structured assistant memory store.
//!
//! Claims are the unit of memory: one durable, reusable assertion about the
//! user or their domain. Every claim carries the evidence it came from, a
//! validity window, and a confidence. Claims are never rewritten in place —
//! a new belief supersedes the old one by closing its window, so the question
//! "why did the assistant believe this back then" stays answerable.

use std::path::Path;

use chrono::DateTime;
use chrono::Utc;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;

/// Failures surfaced by the structured memory store.
#[derive(Debug, thiserror::Error)]
pub enum MemoryDbError {
    /// A claim was offered without evidence. Evidence is mandatory: it is what
    /// makes a memory auditable instead of an unattributed guess.
    #[error("a claim cannot be stored without evidence")]
    MissingEvidence,
    /// The referenced claim does not exist (or was already removed).
    #[error("claim not found")]
    NotFound,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// What a claim is about. Deliberately small: domain packs extend behaviour
/// through `subject` and `scope` rather than by adding kinds here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimKind {
    /// A stable preference of the user.
    Preference,
    /// A stable attribute of the user.
    UserProfile,
    /// A person, organization, or system the user works with.
    Entity,
    /// Something in flight, with a lifecycle.
    Task,
    /// A fact about the user's domain.
    DomainFact,
    /// A method or checklist the user converged on.
    Procedure,
    /// Something the user explicitly corrected.
    Correction,
    /// Something the user asked to have remembered outright, rather than
    /// something the assistant inferred from the conversation.
    Note,
}

/// How strongly the evidence supports the claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

/// What kind of observation an evidence row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceKind {
    Fact,
    Pattern,
    Interpretation,
    Unknown,
}

/// A claim offered for storage, before it exists in the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewClaim {
    pub kind: ClaimKind,
    pub subject: String,
    pub statement: String,
    pub confidence: Confidence,
    /// Where the claim applies, so a true statement cannot be over-generalized.
    pub scope: Option<String>,
    /// What the claim changes about behaviour, when it changes anything.
    pub decision_implication: Option<String>,
    /// When the claim should be re-checked by the assistant.
    pub review_due: Option<DateTime<Utc>>,
    /// When the claim became true in the world. Defaults to the newest
    /// evidence timestamp when the source does not state it.
    pub valid_from: Option<DateTime<Utc>>,
}

impl NewClaim {
    /// The same claim with its subject in the canonical form the store keys on.
    ///
    /// Applied on the way in, so every row on disk uses one spelling per subject
    /// and lookups do not have to guess which spelling a caller meant.
    pub fn normalized(mut self) -> Self {
        self.subject = normalize_key(&self.subject);
        self
    }
}

/// One piece of evidence supporting a claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEvidence {
    pub kind: EvidenceKind,
    pub occurred_at: DateTime<Utc>,
    /// Thread the evidence came from.
    pub source_thread: String,
    /// Location inside that thread (message or item index).
    pub source_locator: String,
    pub excerpt: String,
}

/// A stored claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub id: String,
    pub kind: ClaimKind,
    pub subject: String,
    pub statement: String,
    pub confidence: Confidence,
    pub scope: Option<String>,
    pub decision_implication: Option<String>,
    /// When the claim became true in the world.
    pub valid_from: DateTime<Utc>,
    /// When it stopped being true; `None` while it still holds.
    pub valid_to: Option<DateTime<Utc>>,
    /// The claim that replaced this one, when it was superseded.
    pub superseded_by: Option<String>,
    /// When the assistant learned it (transaction time).
    pub created_at: DateTime<Utc>,
    pub review_due: Option<DateTime<Utc>>,
    pub usage_count: i64,
    pub last_used: Option<DateTime<Utc>>,
    pub pinned: bool,
    /// The claim this one contradicts, while the user has not yet settled which
    /// belief holds.
    ///
    /// A claim under review is stored — it is the newest thing the user said —
    /// but it is held out of retrieval, so the model is never handed two opposite
    /// statements about the same subject and left to pick one.
    pub conflict_with: Option<String>,
}

/// A claim matching a recall query, with the evidence about *how* it matched.
///
/// The match itself is a substring test rather than a full-text query: FTS5's
/// default tokenizer treats a run of Chinese characters as a single token, so
/// "偏好" would not find "用户偏好简短回复", and the trigram tokenizer cannot help
/// either because it needs three characters. At personal scale (hundreds of
/// short claims) a substring scan costs under a millisecond and works for both
/// languages, so the scorer is given the facts and does the ranking in Rust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimHit {
    pub claim: Claim,
    /// How many of the query's terms appear in the claim's subject or statement.
    pub matched_terms: usize,
    /// Whether a term matched the subject, which usually means the query is about
    /// that subject rather than merely sharing words with a statement.
    pub subject_match: bool,
}

/// The outcome of one candidate lookup: the hits, plus the query's term count.
///
/// `term_count` travels separately because it is the denominator of match
/// quality — a claim hitting one of two terms is a better answer than one hitting
/// one of eight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecallMatches {
    pub hits: Vec<ClaimHit>,
    pub term_count: usize,
}

/// A stored evidence row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub kind: EvidenceKind,
    pub occurred_at: DateTime<Utc>,
    pub source_thread: String,
    pub source_locator: String,
    pub excerpt: String,
}

/// SQLite-backed structured memory.
pub struct MemoryDb {
    pool: SqlitePool,
}

/// What a health check found in the store.
///
/// Every field is a count or a boolean so the panel can show it without knowing
/// SQL, and so a test can assert on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorReport {
    /// Claims of any state.
    pub claims_total: i64,
    /// Claims in force (`valid_to` open, not removed by the user).
    pub active: i64,
    /// Claims whose validity window has been closed.
    pub superseded: i64,
    /// Claims the user removed; retained for the audit window.
    pub deleted: i64,
    /// Active claims past their `review_due` date. Not an error: these are simply
    /// no longer injected until the user or a later pass revisits them.
    pub overdue: i64,
    /// Claims held out of retrieval while the user decides which of two
    /// contradicting statements holds. Also not an error.
    pub awaiting_review: i64,
    /// Evidence rows whose claim is gone.
    pub orphan_evidence: i64,
    /// Claims closed before they opened, which means a write went wrong.
    pub invalid_windows: i64,
    /// Rows in the full-text index.
    pub index_rows: i64,
    /// Whether the index still mirrors the claims table 1:1.
    pub index_in_sync: bool,
    /// Whether SQLite reports the file as structurally sound.
    pub integrity_ok: bool,
}

impl DoctorReport {
    /// Whether anything needs repair or attention.
    pub fn needs_attention(&self) -> bool {
        !self.index_in_sync
            || !self.integrity_ok
            || self.orphan_evidence > 0
            || self.invalid_windows > 0
    }
}

/// Canonical form of a value used as a matching key.
///
/// `subject` comes from a model as free text, so `User`, `user ` and `user` have
/// to collapse to one key: otherwise supersede, consolidation and recall each miss
/// the claim they were meant to find.
pub(crate) fn normalize_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// File name of the store inside the assistant memory root.
pub const DB_FILENAME: &str = "memory.sqlite";

/// File name of the JSON view the settings panel reads.
pub const VIEW_FILENAME: &str = "memory_view.json";

/// Evidence `source_thread` marking a claim that came from an ad-hoc note file
/// rather than from a conversation.
pub const NOTE_SOURCE_THREAD: &str = "note";

/// Statements run once per connection pool to make the store usable.
const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS claims (\
        id TEXT PRIMARY KEY, kind TEXT NOT NULL, subject TEXT NOT NULL, \
        statement TEXT NOT NULL, confidence TEXT NOT NULL, scope TEXT, \
        decision_implication TEXT, valid_from INTEGER NOT NULL, valid_to INTEGER, \
        superseded_by TEXT, created_at INTEGER NOT NULL, review_due INTEGER, \
        usage_count INTEGER NOT NULL DEFAULT 0, last_used INTEGER, \
        pinned INTEGER NOT NULL DEFAULT 0, deleted_at INTEGER, conflict_with TEXT)",
    "CREATE INDEX IF NOT EXISTS idx_claims_current ON claims(deleted_at, valid_to)",
    "CREATE INDEX IF NOT EXISTS idx_claims_subject ON claims(subject, valid_from)",
    "CREATE TABLE IF NOT EXISTS evidence (\
        id TEXT PRIMARY KEY, claim_id TEXT NOT NULL, kind TEXT NOT NULL, \
        occurred_at INTEGER NOT NULL, source_thread TEXT NOT NULL, \
        source_locator TEXT NOT NULL, excerpt TEXT NOT NULL, created_at INTEGER NOT NULL)",
    "CREATE INDEX IF NOT EXISTS idx_evidence_claim ON evidence(claim_id)",
    "CREATE VIRTUAL TABLE IF NOT EXISTS claims_fts USING fts5(claim_id UNINDEXED, subject, statement)",
    // The FTS index is a 1:1 mirror of `claims`; the triggers keep it that way.
    // Rebuilding by hand is what let the index drift before, so the invariant is
    // `COUNT(claims_fts) == COUNT(claims)` and `repair_fts_index` restores it.
    "CREATE TRIGGER IF NOT EXISTS claims_fts_insert AFTER INSERT ON claims BEGIN \
        INSERT INTO claims_fts(claim_id, subject, statement) \
        VALUES (new.id, new.subject, new.statement); \
     END",
    // Only the two indexed columns are copied. Closing a validity window
    // (`supersede`) must not rewrite the index row.
    "CREATE TRIGGER IF NOT EXISTS claims_fts_update AFTER UPDATE OF subject, statement ON claims BEGIN \
        DELETE FROM claims_fts WHERE claim_id = old.id; \
        INSERT INTO claims_fts(claim_id, subject, statement) \
        VALUES (new.id, new.subject, new.statement); \
     END",
    "CREATE TRIGGER IF NOT EXISTS claims_fts_delete AFTER DELETE ON claims BEGIN \
        DELETE FROM claims_fts WHERE claim_id = old.id; \
     END",
];

fn ts(value: DateTime<Utc>) -> i64 {
    value.timestamp()
}

fn from_ts(value: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(value, 0).unwrap_or(DateTime::UNIX_EPOCH)
}

fn from_ts_opt(value: Option<i64>) -> Option<DateTime<Utc>> {
    value.map(from_ts)
}

fn new_id(prefix: &str) -> String {
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{:x}-{seq:x}", Utc::now().timestamp_micros())
}

pub(crate) fn claim_kind_str(kind: ClaimKind) -> &'static str {
    match kind {
        ClaimKind::Preference => "preference",
        ClaimKind::UserProfile => "user_profile",
        ClaimKind::Entity => "entity",
        ClaimKind::Task => "task",
        ClaimKind::DomainFact => "domain_fact",
        ClaimKind::Procedure => "procedure",
        ClaimKind::Correction => "correction",
        ClaimKind::Note => "note",
    }
}

pub(crate) fn claim_kind_from(value: &str) -> ClaimKind {
    match value {
        "user_profile" => ClaimKind::UserProfile,
        "entity" => ClaimKind::Entity,
        "task" => ClaimKind::Task,
        "domain_fact" => ClaimKind::DomainFact,
        "procedure" => ClaimKind::Procedure,
        "correction" => ClaimKind::Correction,
        "note" => ClaimKind::Note,
        _ => ClaimKind::Preference,
    }
}

pub(crate) fn confidence_str(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Low => "low",
        Confidence::Medium => "medium",
        Confidence::High => "high",
    }
}

pub(crate) fn confidence_from(value: &str) -> Confidence {
    match value {
        "low" => Confidence::Low,
        "medium" => Confidence::Medium,
        _ => Confidence::High,
    }
}

fn evidence_kind_str(kind: EvidenceKind) -> &'static str {
    match kind {
        EvidenceKind::Fact => "fact",
        EvidenceKind::Pattern => "pattern",
        EvidenceKind::Interpretation => "interpretation",
        EvidenceKind::Unknown => "unknown",
    }
}

fn evidence_kind_from(value: &str) -> EvidenceKind {
    match value {
        "pattern" => EvidenceKind::Pattern,
        "interpretation" => EvidenceKind::Interpretation,
        "unknown" => EvidenceKind::Unknown,
        _ => EvidenceKind::Fact,
    }
}

impl MemoryDb {
    /// Opens (creating if needed) the store under the assistant memory root.
    pub async fn open(root: &Path) -> Result<Self, MemoryDbError> {
        tokio::fs::create_dir_all(root).await?;
        let options = SqliteConnectOptions::new()
            .filename(root.join(DB_FILENAME))
            .create_if_missing(true);
        Self::connect(options).await
    }

    /// Opens a throwaway store, for tests.
    pub async fn open_in_memory() -> Result<Self, MemoryDbError> {
        let options = SqliteConnectOptions::new().in_memory(true);
        Self::connect(options).await
    }

    async fn connect(options: SqliteConnectOptions) -> Result<Self, MemoryDbError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        let db = Self { pool };
        db.ensure_schema().await?;
        db.migrate_columns().await?;
        db.normalize_stored_subjects().await?;
        db.repair_fts_index().await?;
        Ok(db)
    }

    async fn ensure_schema(&self) -> Result<(), MemoryDbError> {
        for statement in SCHEMA {
            sqlx::query(*statement).execute(&self.pool).await?;
        }
        Ok(())
    }

    /// Adds columns introduced after the first release.
    ///
    /// `CREATE TABLE IF NOT EXISTS` leaves an existing table alone, so a store
    /// written by an older build needs the column added explicitly. Checking
    /// `pragma_table_info` first keeps this idempotent without matching on error
    /// strings.
    async fn migrate_columns(&self) -> Result<(), MemoryDbError> {
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('claims')")
                .fetch_all(&self.pool)
                .await?;
        if !columns.iter().any(|column| column == "conflict_with") {
            tracing::info!("assistant memory: adding the conflict_with column to claims");
            sqlx::query("ALTER TABLE claims ADD COLUMN conflict_with TEXT")
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    /// Rewrites subjects stored before normalization existed.
    ///
    /// Idempotent and cheap on a store that is already canonical: rows that match
    /// their own normalized form are skipped, so the cost is one `SELECT` per open.
    /// The rewrite is done in one transaction; the index follows via the
    /// `claims_fts_update` trigger.
    async fn normalize_stored_subjects(&self) -> Result<(), MemoryDbError> {
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT id, subject FROM claims")
            .fetch_all(&self.pool)
            .await?;
        let updates = rows
            .into_iter()
            .filter_map(|(id, subject)| {
                let normalized = normalize_key(&subject);
                (normalized != subject).then_some((id, normalized))
            })
            .collect::<Vec<_>>();
        if updates.is_empty() {
            return Ok(());
        }

        tracing::info!(
            rows = updates.len(),
            "assistant memory: normalizing stored subjects"
        );
        let mut tx = self.pool.begin().await?;
        for (id, subject) in &updates {
            sqlx::query("UPDATE claims SET subject = ? WHERE id = ?")
                .bind(subject)
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Runs a health check over the store.
    ///
    /// Read-only, so it is safe to call from a settings panel. `overdue` is
    /// reported rather than treated as damage: a claim awaiting review is a
    /// prompt for the user, not a broken row.
    pub async fn doctor(&self) -> Result<DoctorReport, MemoryDbError> {
        let claims_total = self.count("SELECT COUNT(*) FROM claims").await?;
        let index_rows = self.count("SELECT COUNT(*) FROM claims_fts").await?;
        let active = self
            .count("SELECT COUNT(*) FROM claims WHERE valid_to IS NULL AND deleted_at IS NULL")
            .await?;
        let superseded = self
            .count("SELECT COUNT(*) FROM claims WHERE valid_to IS NOT NULL AND deleted_at IS NULL")
            .await?;
        let deleted = self
            .count("SELECT COUNT(*) FROM claims WHERE deleted_at IS NOT NULL")
            .await?;
        let overdue = sqlx::query_scalar(
            "SELECT COUNT(*) FROM claims WHERE valid_to IS NULL AND deleted_at IS NULL \
               AND review_due IS NOT NULL AND review_due <= ?",
        )
        .bind(ts(Utc::now()))
        .fetch_one(&self.pool)
        .await?;
        let awaiting_review = self.awaiting_review_count().await?;
        let orphan_evidence = self
            .count(
                "SELECT COUNT(*) FROM evidence e \
                 WHERE NOT EXISTS (SELECT 1 FROM claims c WHERE c.id = e.claim_id)",
            )
            .await?;
        let invalid_windows = self
            .count(
                "SELECT COUNT(*) FROM claims WHERE valid_to IS NOT NULL AND valid_to < valid_from",
            )
            .await?;

        let integrity_rows: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_all(&self.pool)
            .await?;
        let integrity_ok = integrity_rows.len() == 1 && integrity_rows[0] == "ok";

        Ok(DoctorReport {
            claims_total,
            active,
            superseded,
            deleted,
            overdue,
            awaiting_review,
            orphan_evidence,
            invalid_windows,
            index_rows,
            index_in_sync: index_rows == claims_total,
            integrity_ok,
        })
    }

    async fn count(&self, sql: &'static str) -> Result<i64, MemoryDbError> {
        Ok(sqlx::query_scalar(sql).fetch_one(&self.pool).await?)
    }

    /// Claims held out of retrieval pending the user's decision.
    pub async fn awaiting_review_count(&self) -> Result<i64, MemoryDbError> {
        self.count(
            "SELECT COUNT(*) FROM claims \
             WHERE conflict_with IS NOT NULL AND valid_to IS NULL AND deleted_at IS NULL",
        )
        .await
    }

    /// Whether a note file has already been turned into a claim.
    ///
    /// Keyed on the evidence row, not on the note file: the note stays on disk for
    /// the user to read and edit, so file existence cannot mean "already absorbed".
    pub async fn has_note_claim(&self, filename: &str) -> Result<bool, MemoryDbError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM evidence WHERE source_thread = ? AND source_locator = ?",
        )
        .bind(NOTE_SOURCE_THREAD)
        .bind(filename)
        .fetch_one(&self.pool)
        .await?;
        Ok(count > 0)
    }

    /// Claims whose review date has passed and which are still in force.
    ///
    /// These are excluded from retrieval — that is the existing product decision,
    /// so a belief that may no longer hold is not asserted at the model — but they
    /// are shown to the extraction pass, which is the one place able to notice
    /// that the world moved on and close them.
    pub async fn review_due_claims(
        &self,
        limit: usize,
        now: DateTime<Utc>,
    ) -> Result<Vec<Claim>, MemoryDbError> {
        let rows = sqlx::query(
            "SELECT id, kind, subject, statement, confidence, scope, decision_implication, \
             valid_from, valid_to, superseded_by, created_at, review_due, usage_count, last_used, \
             conflict_with, pinned FROM claims \
             WHERE valid_to IS NULL AND deleted_at IS NULL \
               AND review_due IS NOT NULL AND review_due <= ? \
             ORDER BY review_due ASC, created_at DESC LIMIT ?",
        )
        .bind(ts(now))
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_claim).collect()
    }

    /// Stores a claim the user has not yet ruled on.
    ///
    /// The claim is kept — it is the newest thing the user said, and dropping it
    /// would lose information — but it is marked as contradicting
    /// `conflicts_with` and held out of retrieval until [`Self::keep_claim`] or a
    /// removal settles which belief holds.
    pub async fn insert_conflicting(
        &self,
        claim: NewClaim,
        evidence: Vec<NewEvidence>,
        conflicts_with: &str,
    ) -> Result<String, MemoryDbError> {
        let id = self.insert_claim(claim, evidence).await?;
        sqlx::query("UPDATE claims SET conflict_with = ? WHERE id = ? AND valid_to IS NULL")
            .bind(conflicts_with)
            .bind(&id)
            .execute(&self.pool)
            .await?;
        Ok(id)
    }

    /// Confirms a claim the user has seen, closing whatever it contradicted.
    ///
    /// A claim with no recorded conflict is already settled, so this reports
    /// success without touching anything: the user confirming a memory that was
    /// never in question should not close an unrelated row.
    pub async fn keep_claim(&self, id: &str) -> Result<(), MemoryDbError> {
        let claim = self.load_claim(id).await?;
        let Some(contradicted) = claim.conflict_with.clone() else {
            return Ok(());
        };

        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE claims SET conflict_with = NULL WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE claims SET valid_to = ?, superseded_by = ? WHERE id = ? AND valid_to IS NULL",
        )
        .bind(ts(Utc::now()))
        .bind(id)
        .bind(&contradicted)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Restores the `claims_fts` mirror when it has drifted away from `claims`.
    ///
    /// The triggers keep the two in step for every write this process makes, so a
    /// mismatch means the store was written by an older build (or edited by hand).
    /// Two `COUNT`s per open is cheap and turns "silently wrong search results"
    /// into "self-healed on next open".
    async fn repair_fts_index(&self) -> Result<(), MemoryDbError> {
        let claims: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM claims")
            .fetch_one(&self.pool)
            .await?;
        let indexed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM claims_fts")
            .fetch_one(&self.pool)
            .await?;
        if claims == indexed {
            return Ok(());
        }

        tracing::warn!(
            claims,
            indexed,
            "assistant memory: full-text index was out of step with claims; rebuilding"
        );
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM claims_fts")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO claims_fts(claim_id, subject, statement) \
             SELECT id, subject, statement FROM claims",
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Stores a new claim together with the evidence it came from.
    pub async fn insert_claim(
        &self,
        claim: NewClaim,
        evidence: Vec<NewEvidence>,
    ) -> Result<String, MemoryDbError> {
        if evidence.is_empty() {
            return Err(MemoryDbError::MissingEvidence);
        }
        let claim = claim.normalized();
        let id = new_id("claim");
        let now = Utc::now();
        let valid_from = claim.valid_from.unwrap_or_else(|| {
            evidence
                .iter()
                .map(|item| item.occurred_at)
                .max()
                .unwrap_or(now)
        });

        let mut tx = self.pool.begin().await?;
        insert_claim_row(&mut tx, &id, &claim, valid_from, now).await?;
        insert_evidence_rows(&mut tx, &id, &evidence, now).await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Replaces a claim: the old one keeps its history but stops being current.
    pub async fn supersede(
        &self,
        old_id: &str,
        claim: NewClaim,
        evidence: Vec<NewEvidence>,
        at: DateTime<Utc>,
    ) -> Result<String, MemoryDbError> {
        if evidence.is_empty() {
            return Err(MemoryDbError::MissingEvidence);
        }
        let claim = claim.normalized();
        self.load_claim(old_id).await?;
        let id = new_id("claim");
        let now = Utc::now();
        let valid_from = claim.valid_from.unwrap_or(at);

        let mut tx = self.pool.begin().await?;
        insert_claim_row(&mut tx, &id, &claim, valid_from, now).await?;
        insert_evidence_rows(&mut tx, &id, &evidence, now).await?;
        sqlx::query(
            "UPDATE claims SET valid_to = ?, superseded_by = ? WHERE id = ? AND valid_to IS NULL",
        )
        .bind(ts(at))
        .bind(&id)
        .bind(old_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }
}

async fn insert_claim_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    id: &str,
    claim: &NewClaim,
    valid_from: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<(), MemoryDbError> {
    sqlx::query(
        "INSERT INTO claims (id, kind, subject, statement, confidence, scope, \
         decision_implication, valid_from, created_at, review_due) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(claim_kind_str(claim.kind))
    .bind(&claim.subject)
    .bind(&claim.statement)
    .bind(confidence_str(claim.confidence))
    .bind(claim.scope.as_deref())
    .bind(claim.decision_implication.as_deref())
    .bind(ts(valid_from))
    .bind(ts(now))
    .bind(claim.review_due.map(ts))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_evidence_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    claim_id: &str,
    evidence: &[NewEvidence],
    now: DateTime<Utc>,
) -> Result<(), MemoryDbError> {
    for item in evidence {
        sqlx::query(
            "INSERT INTO evidence (id, claim_id, kind, occurred_at, source_thread, \
             source_locator, excerpt, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(new_id("ev"))
        .bind(claim_id)
        .bind(evidence_kind_str(item.kind))
        .bind(ts(item.occurred_at))
        .bind(&item.source_thread)
        .bind(&item.source_locator)
        .bind(&item.excerpt)
        .bind(ts(now))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Most terms one query is expanded into. A long message produces one bigram per
/// adjacent character pair, so the list is sampled down rather than truncated —
/// truncating would make recall depend only on the opening words.
const MAX_RECALL_TERMS: usize = 64;
/// Shortest Latin term worth matching: a single letter matches nearly everything.
const MIN_LATIN_TERM_CHARS: usize = 2;

/// Splits a query into the terms used for substring matching.
///
/// Latin words are kept whole — `instr` needs a contiguous substring, so cutting
/// `concisely` into pieces would only make it match less. A run of CJK characters
/// is split into overlapping bigrams instead, because a Chinese query rarely
/// repeats the exact phrase a claim happens to store: "偏好" has to find
/// "用户偏好简短回复".
fn recall_terms(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut latin = String::new();
    let mut cjk: Vec<char> = Vec::new();

    for ch in query.chars() {
        if is_cjk_char(ch) {
            flush_latin_term(&mut terms, &mut latin);
            cjk.push(ch);
            continue;
        }
        flush_cjk_terms(&mut terms, &mut cjk);
        if ch.is_alphanumeric() {
            latin.extend(ch.to_lowercase());
        } else {
            flush_latin_term(&mut terms, &mut latin);
        }
    }
    flush_cjk_terms(&mut terms, &mut cjk);
    flush_latin_term(&mut terms, &mut latin);

    trim_terms(terms)
}

fn flush_latin_term(terms: &mut Vec<String>, latin: &mut String) {
    if latin.chars().count() >= MIN_LATIN_TERM_CHARS {
        push_term(terms, std::mem::take(latin));
    } else {
        latin.clear();
    }
}

fn flush_cjk_terms(terms: &mut Vec<String>, run: &mut Vec<char>) {
    match run.len() {
        0 => {}
        // A lone character is still a word in these scripts.
        1 => push_term(terms, run[0].to_string()),
        _ => {
            for pair in run.windows(2) {
                push_term(terms, pair.iter().collect());
            }
        }
    }
    run.clear();
}

fn push_term(terms: &mut Vec<String>, term: String) {
    if term.is_empty() {
        return;
    }
    if !terms.contains(&term) {
        terms.push(term);
    }
}

/// Keeps at most [`MAX_RECALL_TERMS`], spread evenly across the query.
fn trim_terms(terms: Vec<String>) -> Vec<String> {
    if terms.len() <= MAX_RECALL_TERMS {
        return terms;
    }
    let step = terms.len() as f64 / MAX_RECALL_TERMS as f64;
    (0..MAX_RECALL_TERMS)
        .map(|index| terms[(index as f64 * step) as usize].clone())
        .collect()
}

/// Whether a character belongs to a script written without word separators.
pub(crate) fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3040..=0x30FF      // kana
            | 0x3400..=0x4DBF    // CJK extension A
            | 0x4E00..=0x9FFF    // CJK unified ideographs
            | 0xAC00..=0xD7AF    // hangul syllables
            | 0xF900..=0xFAFF    // compatibility ideographs
            | 0x20000..=0x2FA1F  // CJK extensions B..F
    )
}

/// Turns free text into a safe FTS5 query: every word must appear.
fn fts_query_for(query: &str) -> String {
    query
        .split_whitespace()
        .map(|word| format!("\"{}\"", word.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

impl MemoryDb {
    /// Closes a claim in favour of one that already covers it.
    ///
    /// Consolidation learns nothing new: one claim is simply subsumed by
    /// another, so no row is stored — only the closed window is written.
    pub async fn mark_superseded_by(
        &self,
        old_id: &str,
        covering_id: &str,
        at: DateTime<Utc>,
    ) -> Result<(), MemoryDbError> {
        let result = sqlx::query(
            "UPDATE claims SET valid_to = ?, superseded_by = ?              WHERE id = ? AND valid_to IS NULL AND deleted_at IS NULL",
        )
        .bind(ts(at))
        .bind(covering_id)
        .bind(old_id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(MemoryDbError::NotFound);
        }
        Ok(())
    }

    /// Claims that are current and not removed by the user.
    pub async fn active_claims(&self, limit: usize) -> Result<Vec<Claim>, MemoryDbError> {
        let rows = sqlx::query(
            "SELECT id, kind, subject, statement, confidence, scope, decision_implication, \
             valid_from, valid_to, superseded_by, created_at, review_due, usage_count, last_used, \
             conflict_with, pinned FROM claims WHERE valid_to IS NULL AND deleted_at IS NULL \
             ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_claim).collect()
    }

    /// Every claim the user can still see, including superseded history.
    pub async fn visible_claims(&self) -> Result<Vec<Claim>, MemoryDbError> {
        let rows = sqlx::query(
            "SELECT id, kind, subject, statement, confidence, scope, decision_implication, \
             valid_from, valid_to, superseded_by, created_at, review_due, usage_count, last_used, \
             conflict_with, pinned FROM claims WHERE deleted_at IS NULL ORDER BY created_at DESC, id DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_claim).collect()
    }

    /// Loads one claim by id, including superseded ones.
    pub async fn load_claim(&self, id: &str) -> Result<Claim, MemoryDbError> {
        let row = sqlx::query(
            "SELECT id, kind, subject, statement, confidence, scope, decision_implication, \
             valid_from, valid_to, superseded_by, created_at, review_due, usage_count, last_used, \
             conflict_with, pinned FROM claims WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(MemoryDbError::NotFound)?;
        row_to_claim(&row)
    }

    /// Evidence supporting a claim.
    pub async fn evidence_for(&self, claim_id: &str) -> Result<Vec<Evidence>, MemoryDbError> {
        let rows = sqlx::query(
            "SELECT kind, occurred_at, source_thread, source_locator, excerpt FROM evidence \
             WHERE claim_id = ? ORDER BY occurred_at ASC, id ASC",
        )
        .bind(claim_id)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_evidence).collect()
    }

    /// What was believed about `subject` at a given moment.
    pub async fn claims_valid_at(
        &self,
        subject: &str,
        at: DateTime<Utc>,
    ) -> Result<Vec<Claim>, MemoryDbError> {
        // Callers spell subjects however their model wrote them; the store keeps
        // one spelling, so the lookup has to be canonicalized the same way.
        let subject = normalize_key(subject);
        let rows = sqlx::query(
            "SELECT id, kind, subject, statement, confidence, scope, decision_implication, \
             valid_from, valid_to, superseded_by, created_at, review_due, usage_count, last_used, \
             conflict_with, pinned FROM claims WHERE subject = ? AND deleted_at IS NULL AND valid_from <= ? \
               AND (valid_to IS NULL OR valid_to > ?) \
             ORDER BY valid_from DESC, created_at DESC",
        )
        .bind(subject)
        .bind(ts(at))
        .bind(ts(at))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_claim).collect()
    }

    /// Claims currently in force for one subject.
    ///
    /// Reconciliation asks this per candidate: a claim only ever competes with
    /// the beliefs held about its own subject.
    pub async fn active_claims_for_subject(
        &self,
        subject: &str,
    ) -> Result<Vec<Claim>, MemoryDbError> {
        self.claims_valid_at(subject, Utc::now()).await
    }

    /// Full-text search over current claims.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<Claim>, MemoryDbError> {
        let fts_query = fts_query_for(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT c.id, c.kind, c.subject, c.statement, c.confidence, c.scope, \
             c.decision_implication, c.valid_from, c.valid_to, c.superseded_by, c.created_at, \
             c.review_due, c.usage_count, c.last_used, c.conflict_with, c.pinned FROM claims c \
             JOIN claims_fts f ON f.claim_id = c.id \
             WHERE claims_fts MATCH ? AND c.valid_to IS NULL AND c.deleted_at IS NULL \
             ORDER BY bm25(claims_fts), c.created_at DESC LIMIT ?",
        )
        .bind(fts_query)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_claim).collect()
    }

    /// Claims eligible for injection into a prompt, best first.
    ///
    /// Order is user intent first (`pinned`), then confidence, then recency.
    /// `usage_count` is deliberately *not* part of the order: the injection path
    /// records usage, so ranking by it would turn the first twelve claims into a
    /// self-reinforcing clique that a newly learned memory could never enter.
    pub async fn injection_candidates(
        &self,
        limit: usize,
        now: DateTime<Utc>,
    ) -> Result<Vec<Claim>, MemoryDbError> {
        let rows = sqlx::query(
            "SELECT id, kind, subject, statement, confidence, scope, decision_implication, \
             valid_from, valid_to, superseded_by, created_at, review_due, usage_count, last_used, \
             conflict_with, pinned FROM claims WHERE valid_to IS NULL AND deleted_at IS NULL \
               AND (review_due IS NULL OR review_due > ?) \
               AND conflict_with IS NULL \
             ORDER BY pinned DESC, \
               CASE confidence WHEN 'high' THEN 0 WHEN 'medium' THEN 1 ELSE 2 END ASC, \
               created_at DESC, id DESC LIMIT ?",
        )
        .bind(ts(now))
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_claim).collect()
    }

    /// Claims whose subject or statement contains a term of `query`.
    ///
    /// Same freshness rules as injection: superseded, user-removed, and
    /// overdue-for-review claims are not recalled — a claim awaiting review must
    /// not keep being re-injected on the strength of the review it has not had.
    ///
    /// A query with no usable terms returns nothing rather than everything, so an
    /// empty user message cannot spray the whole store into the prompt. Rows come
    /// back newest first; ordering them by relevance is the caller's job
    /// ([`crate::recall`]), which is where the weighting lives.
    pub async fn recall_candidates(
        &self,
        query: &str,
        limit: usize,
        now: DateTime<Utc>,
    ) -> Result<RecallMatches, MemoryDbError> {
        let terms = recall_terms(query);
        if terms.is_empty() || limit == 0 {
            return Ok(RecallMatches {
                hits: Vec::new(),
                term_count: terms.len(),
            });
        }

        let mut builder: sqlx::QueryBuilder<sqlx::Sqlite> = sqlx::QueryBuilder::new(
            "SELECT id, kind, subject, statement, confidence, scope, decision_implication, \
             valid_from, valid_to, superseded_by, created_at, review_due, usage_count, last_used, \
             conflict_with, pinned FROM claims WHERE valid_to IS NULL AND deleted_at IS NULL \
               AND (review_due IS NULL OR review_due > ",
        );
        builder.push_bind(ts(now));
        // A claim under review is not recalled: the user has not yet said which
        // of two contradicting statements holds.
        builder.push(") AND conflict_with IS NULL AND (");
        for (index, term) in terms.iter().enumerate() {
            if index > 0 {
                builder.push(" OR ");
            }
            builder.push("instr(lower(subject), ");
            builder.push_bind(term.clone());
            builder.push(") > 0 OR instr(lower(statement), ");
            builder.push_bind(term.clone());
            builder.push(") > 0");
        }
        builder.push(") ORDER BY created_at DESC, id DESC LIMIT ");
        builder.push_bind(limit as i64);
        let rows = builder.build().fetch_all(&self.pool).await?;

        let mut hits = Vec::with_capacity(rows.len());
        for row in &rows {
            let claim = row_to_claim(row)?;
            let subject = claim.subject.to_lowercase();
            let statement = claim.statement.to_lowercase();
            let matched_terms = terms
                .iter()
                .filter(|term| subject.contains(term.as_str()) || statement.contains(term.as_str()))
                .count();
            let subject_match = terms.iter().any(|term| subject.contains(term.as_str()));
            hits.push(ClaimHit {
                claim,
                matched_terms,
                subject_match,
            });
        }
        Ok(RecallMatches {
            hits,
            term_count: terms.len(),
        })
    }
}

impl MemoryDb {
    /// Records that claims were handed to the model.
    ///
    /// This counts *exposure*, not proven use: the store cannot see whether the
    /// model acted on a claim, but both the injection path and the recall tool
    /// know they put one in front of it. The counter is shown in the settings
    /// view and deliberately kept out of the injection order — see
    /// [`Self::injection_candidates`] for why.
    pub async fn record_usage(&self, ids: &[String]) -> Result<(), MemoryDbError> {
        if ids.is_empty() {
            return Ok(());
        }
        let now = ts(Utc::now());
        let mut tx = self.pool.begin().await?;
        for id in ids {
            sqlx::query(
                "UPDATE claims SET usage_count = usage_count + 1, last_used = ? WHERE id = ?",
            )
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Pins or unpins a claim (pinned claims survive the injection budget).
    pub async fn set_pinned(&self, id: &str, pinned: bool) -> Result<(), MemoryDbError> {
        let result =
            sqlx::query("UPDATE claims SET pinned = ? WHERE id = ? AND deleted_at IS NULL")
                .bind(i64::from(pinned))
                .bind(id)
                .execute(&self.pool)
                .await?;
        if result.rows_affected() == 0 {
            return Err(MemoryDbError::NotFound);
        }
        Ok(())
    }

    /// Removes a claim at the user's request. Retrieval and the view stop
    /// showing it immediately; the row is retained for a short audit window.
    pub async fn delete_claim(&self, id: &str) -> Result<(), MemoryDbError> {
        let result =
            sqlx::query("UPDATE claims SET deleted_at = ? WHERE id = ? AND deleted_at IS NULL")
                .bind(ts(Utc::now()))
                .bind(id)
                .execute(&self.pool)
                .await?;
        if result.rows_affected() == 0 {
            return Err(MemoryDbError::NotFound);
        }
        Ok(())
    }

    /// Writes the JSON view the settings panel reads.
    pub async fn render_view(&self, path: &Path) -> Result<(), MemoryDbError> {
        let claims = self.visible_claims().await?;
        let health = self.doctor().await?;
        let mut items = Vec::with_capacity(claims.len());
        let mut active = 0_u64;
        let mut superseded = 0_u64;
        for claim in &claims {
            if claim.valid_to.is_none() {
                active += 1;
            } else {
                superseded += 1;
            }
            let evidence = self.evidence_for(&claim.id).await?;
            items.push(claim_to_json(claim, &evidence));
        }

        let payload = serde_json::json!({
            "claims": items,
            "stats": {
                "active": active,
                "superseded": superseded,
                "awaiting_review": health.awaiting_review,
                "overdue": health.overdue,
            },
            // The same checks `memory doctor` runs, so "is memory working?" is
            // answerable from the panel without opening the database.
            "health": {
                "index_in_sync": health.index_in_sync,
                "integrity_ok": health.integrity_ok,
                "orphan_evidence": health.orphan_evidence,
                "invalid_windows": health.invalid_windows,
                "needs_attention": health.needs_attention(),
            },
        });
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(path, serde_json::to_string_pretty(&payload)?).await?;
        Ok(())
    }
}

fn claim_to_json(claim: &Claim, evidence: &[Evidence]) -> serde_json::Value {
    let evidence = evidence
        .iter()
        .map(|item| {
            serde_json::json!({
                "kind": evidence_kind_str(item.kind),
                "occurred_at": item.occurred_at.to_rfc3339(),
                "source_thread": item.source_thread,
                "source_locator": item.source_locator,
                "excerpt": item.excerpt,
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "id": claim.id,
        "kind": claim_kind_str(claim.kind),
        "subject": claim.subject,
        "statement": claim.statement,
        "confidence": confidence_str(claim.confidence),
        "scope": claim.scope,
        "decision_implication": claim.decision_implication,
        "valid_from": claim.valid_from.to_rfc3339(),
        "valid_to": claim.valid_to.map(|value| value.to_rfc3339()),
        "superseded_by": claim.superseded_by,
        "created_at": claim.created_at.to_rfc3339(),
        "review_due": claim.review_due.map(|value| value.to_rfc3339()),
        "usage_count": claim.usage_count,
        "pinned": claim.pinned,
        "conflict_with": claim.conflict_with,
        "evidence": evidence,
    })
}

fn row_to_claim(row: &sqlx::sqlite::SqliteRow) -> Result<Claim, MemoryDbError> {
    use sqlx::Row;
    Ok(Claim {
        id: row.try_get("id")?,
        kind: claim_kind_from(row.try_get::<String, _>("kind")?.as_str()),
        subject: row.try_get("subject")?,
        statement: row.try_get("statement")?,
        confidence: confidence_from(row.try_get::<String, _>("confidence")?.as_str()),
        scope: row.try_get("scope")?,
        decision_implication: row.try_get("decision_implication")?,
        valid_from: from_ts(row.try_get("valid_from")?),
        valid_to: from_ts_opt(row.try_get("valid_to")?),
        superseded_by: row.try_get("superseded_by")?,
        created_at: from_ts(row.try_get("created_at")?),
        review_due: from_ts_opt(row.try_get("review_due")?),
        usage_count: row.try_get("usage_count")?,
        last_used: from_ts_opt(row.try_get("last_used")?),
        pinned: row.try_get::<i64, _>("pinned")? != 0,
        conflict_with: row.try_get("conflict_with")?,
    })
}

fn row_to_evidence(row: &sqlx::sqlite::SqliteRow) -> Result<Evidence, MemoryDbError> {
    use sqlx::Row;
    Ok(Evidence {
        kind: evidence_kind_from(row.try_get::<String, _>("kind")?.as_str()),
        occurred_at: from_ts(row.try_get("occurred_at")?),
        source_thread: row.try_get("source_thread")?,
        source_locator: row.try_get("source_locator")?,
        excerpt: row.try_get("excerpt")?,
    })
}

#[cfg(test)]
#[path = "memory_db_tests.rs"]
mod tests;
