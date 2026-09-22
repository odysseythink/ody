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

/// File name of the store inside the assistant memory root.
pub const DB_FILENAME: &str = "memory.sqlite";

/// File name of the JSON view the settings panel reads.
pub const VIEW_FILENAME: &str = "memory_view.json";

/// Statements run once per connection pool to make the store usable.
const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS claims (\
        id TEXT PRIMARY KEY, kind TEXT NOT NULL, subject TEXT NOT NULL, \
        statement TEXT NOT NULL, confidence TEXT NOT NULL, scope TEXT, \
        decision_implication TEXT, valid_from INTEGER NOT NULL, valid_to INTEGER, \
        superseded_by TEXT, created_at INTEGER NOT NULL, review_due INTEGER, \
        usage_count INTEGER NOT NULL DEFAULT 0, last_used INTEGER, \
        pinned INTEGER NOT NULL DEFAULT 0, deleted_at INTEGER)",
    "CREATE INDEX IF NOT EXISTS idx_claims_current ON claims(deleted_at, valid_to)",
    "CREATE INDEX IF NOT EXISTS idx_claims_subject ON claims(subject, valid_from)",
    "CREATE TABLE IF NOT EXISTS evidence (\
        id TEXT PRIMARY KEY, claim_id TEXT NOT NULL, kind TEXT NOT NULL, \
        occurred_at INTEGER NOT NULL, source_thread TEXT NOT NULL, \
        source_locator TEXT NOT NULL, excerpt TEXT NOT NULL, created_at INTEGER NOT NULL)",
    "CREATE INDEX IF NOT EXISTS idx_evidence_claim ON evidence(claim_id)",
    "CREATE VIRTUAL TABLE IF NOT EXISTS claims_fts USING fts5(claim_id UNINDEXED, subject, statement)",
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
        Ok(db)
    }

    async fn ensure_schema(&self) -> Result<(), MemoryDbError> {
        for statement in SCHEMA {
            sqlx::query(*statement).execute(&self.pool).await?;
        }
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
        insert_fts_row(&mut tx, &id, &claim).await?;
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
        self.load_claim(old_id).await?;
        let id = new_id("claim");
        let now = Utc::now();
        let valid_from = claim.valid_from.unwrap_or(at);

        let mut tx = self.pool.begin().await?;
        insert_claim_row(&mut tx, &id, &claim, valid_from, now).await?;
        insert_evidence_rows(&mut tx, &id, &evidence, now).await?;
        insert_fts_row(&mut tx, &id, &claim).await?;
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

async fn insert_fts_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    claim_id: &str,
    claim: &NewClaim,
) -> Result<(), MemoryDbError> {
    sqlx::query("INSERT INTO claims_fts (claim_id, subject, statement) VALUES (?, ?, ?)")
        .bind(claim_id)
        .bind(&claim.subject)
        .bind(&claim.statement)
        .execute(&mut **tx)
        .await?;
    Ok(())
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
             pinned FROM claims WHERE valid_to IS NULL AND deleted_at IS NULL \
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
             pinned FROM claims WHERE deleted_at IS NULL ORDER BY created_at DESC, id DESC",
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
             pinned FROM claims WHERE id = ? AND deleted_at IS NULL",
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
        let rows = sqlx::query(
            "SELECT id, kind, subject, statement, confidence, scope, decision_implication, \
             valid_from, valid_to, superseded_by, created_at, review_due, usage_count, last_used, \
             pinned FROM claims WHERE subject = ? AND deleted_at IS NULL AND valid_from <= ? \
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
             c.review_due, c.usage_count, c.last_used, c.pinned FROM claims c \
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
    pub async fn injection_candidates(
        &self,
        limit: usize,
        now: DateTime<Utc>,
    ) -> Result<Vec<Claim>, MemoryDbError> {
        let rows = sqlx::query(
            "SELECT id, kind, subject, statement, confidence, scope, decision_implication, \
             valid_from, valid_to, superseded_by, created_at, review_due, usage_count, last_used, \
             pinned FROM claims WHERE valid_to IS NULL AND deleted_at IS NULL \
               AND (review_due IS NULL OR review_due > ?) \
             ORDER BY pinned DESC, usage_count DESC, COALESCE(last_used, created_at) DESC, \
               created_at DESC LIMIT ?",
        )
        .bind(ts(now))
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_claim).collect()
    }
}

impl MemoryDb {
    /// Records that claims were actually used in a reply.
    pub async fn record_usage(&self, ids: &[String]) -> Result<(), MemoryDbError> {
        let now = ts(Utc::now());
        for id in ids {
            sqlx::query(
                "UPDATE claims SET usage_count = usage_count + 1, last_used = ? WHERE id = ?",
            )
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await?;
        }
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
            "stats": { "active": active, "superseded": superseded },
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
