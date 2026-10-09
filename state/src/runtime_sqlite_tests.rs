//! Regression coverage for SQLite pool initialization under competing locks
//! and startup corruption recovery, ported from upstream codex `state` crate
//! (`sqlite_tests.rs` / `sqlite/validation_tests.rs`, fixes #49102 + #49701).

use super::StateRuntime;
use super::logs_db_path;
use super::open_logs_sqlite;
use super::open_state_sqlite;
use super::state_db_path;
use super::test_support::unique_temp_dir;
use crate::migrations::LOGS_MIGRATOR;
use crate::migrations::runtime_logs_migrator;
use crate::migrations::runtime_state_migrator;
use crate::sqlite_validation::QuickCheckOutcome;
use crate::sqlite_validation::SqliteQuickCheckManager;
use crate::sqlite_validation::quick_check;
use pretty_assertions::assert_eq;
use sqlx::sqlite::SqliteAutoVacuum;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqliteJournalMode;
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

#[derive(Debug, Eq, PartialEq)]
struct CorruptionEvent {
    db: String,
}

#[derive(Default)]
struct CorruptionTelemetry(Mutex<Vec<CorruptionEvent>>);

impl CorruptionTelemetry {
    fn events(&self) -> Vec<CorruptionEvent> {
        self.0
            .lock()
            .expect("telemetry lock")
            .iter()
            .map(|event| CorruptionEvent {
                db: event.db.clone(),
            })
            .collect()
    }
}

impl crate::DbTelemetry for CorruptionTelemetry {
    fn counter(&self, name: &str, _inc: i64, tags: &[(&str, &str)]) {
        if name != crate::DB_CORRUPTION_METRIC {
            return;
        }
        let db = tags
            .iter()
            .find(|(key, _)| *key == "db")
            .map(|(_, value)| (*value).to_string())
            .expect("corruption metric db tag");
        self.0
            .lock()
            .expect("telemetry lock")
            .push(CorruptionEvent { db });
    }

    fn record_duration(&self, _name: &str, _duration: Duration, _tags: &[(&str, &str)]) {}
}

#[tokio::test]
async fn open_sqlite_initializes_fresh_database_settings() -> anyhow::Result<()> {
    let ody_home = unique_temp_dir();
    tokio::fs::create_dir_all(&ody_home).await?;
    let database_path = logs_db_path(ody_home.as_path());

    let pool = open_logs_sqlite(
        &database_path,
        &runtime_logs_migrator(),
        /*telemetry_override*/ None,
    )
    .await?;

    let journal_mode = sqlx::query_scalar::<_, String>("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await?;
    let auto_vacuum = sqlx::query_scalar::<_, i64>("PRAGMA auto_vacuum")
        .fetch_one(&pool)
        .await?;

    assert_eq!(
        (journal_mode, auto_vacuum),
        ("wal".to_string(), SqliteAutoVacuum::Incremental as i64)
    );
    pool.close().await;
    let _ = tokio::fs::remove_dir_all(ody_home).await;
    Ok(())
}

#[tokio::test]
async fn open_sqlite_preserves_full_auto_vacuum_under_write_lock() -> anyhow::Result<()> {
    let ody_home = unique_temp_dir();
    tokio::fs::create_dir_all(&ody_home).await?;
    let database_path = logs_db_path(ody_home.as_path());
    // An existing database created with FULL auto-vacuum and a migrated schema.
    let existing_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&database_path)
                .create_if_missing(true)
                .auto_vacuum(SqliteAutoVacuum::Full)
                .journal_mode(SqliteJournalMode::Wal),
        )
        .await?;
    LOGS_MIGRATOR.run(&existing_pool).await?;

    let mut lock_holder = existing_pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *lock_holder)
        .await?;

    // Opening must not take the writer lock just to restate an auto-vacuum
    // mode the database cannot adopt without a VACUUM anyway.
    let pool = open_logs_sqlite(
        &database_path,
        &runtime_logs_migrator(),
        /*telemetry_override*/ None,
    )
    .await?;
    let auto_vacuum = sqlx::query_scalar::<_, i64>("PRAGMA auto_vacuum")
        .fetch_one(&pool)
        .await?;

    assert_eq!(auto_vacuum, SqliteAutoVacuum::Full as i64);
    sqlx::query("ROLLBACK").execute(&mut *lock_holder).await?;
    drop(lock_holder);
    pool.close().await;
    existing_pool.close().await;
    let _ = tokio::fs::remove_dir_all(ody_home).await;
    Ok(())
}

#[tokio::test]
async fn open_sqlite_surfaces_wal_conversion_lock_error() -> anyhow::Result<()> {
    let ody_home = unique_temp_dir();
    tokio::fs::create_dir_all(&ody_home).await?;
    let database_path = logs_db_path(ody_home.as_path());
    let existing_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&database_path)
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Delete),
        )
        .await?;
    sqlx::query("CREATE TABLE existing (id INTEGER PRIMARY KEY)")
        .execute(&existing_pool)
        .await?;
    let mut lock_holder = existing_pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *lock_holder)
        .await?;

    let result = open_logs_sqlite(
        &database_path,
        &runtime_logs_migrator(),
        /*telemetry_override*/ None,
    )
    .await;
    sqlx::query("ROLLBACK").execute(&mut *lock_holder).await?;
    drop(lock_holder);
    existing_pool.close().await;

    // WAL conversion requires the writer lock; the original lock error must
    // reach the caller instead of being masked as a pool timeout.
    let error = result.expect_err("WAL conversion requires the writer lock");
    assert!(
        !error.chain().any(|cause| {
            matches!(
                cause.downcast_ref::<sqlx::Error>(),
                Some(sqlx::Error::PoolTimedOut)
            )
        }),
        "lock error must not be masked as PoolTimedOut: {error}"
    );
    assert!(
        crate::sqlite_error_detail_is_lock(&error.to_string()),
        "expected a lock error, got: {error}"
    );
    // Failure must release the connection so a later startup can succeed.
    open_logs_sqlite(
        &database_path,
        &runtime_logs_migrator(),
        /*telemetry_override*/ None,
    )
    .await?
    .close()
    .await;
    let _ = tokio::fs::remove_dir_all(ody_home).await;
    Ok(())
}

#[tokio::test]
async fn corrupt_logs_db_is_backed_up_and_rebuilt_during_runtime_init() -> anyhow::Result<()> {
    let ody_home = unique_temp_dir();
    tokio::fs::create_dir_all(&ody_home).await?;
    // First init creates the real databases; keep one state row to prove the
    // rebuild only discards the damaged database.
    let runtime = StateRuntime::init(ody_home.clone(), "test-provider".to_string()).await?;
    runtime.close().await;
    let state_pool = open_state_sqlite(
        &state_db_path(ody_home.as_path()),
        &runtime_state_migrator(),
        /*telemetry_override*/ None,
    )
    .await?;
    sqlx::query("CREATE TABLE preserved(value); INSERT INTO preserved VALUES ('keep me')")
        .execute(&state_pool)
        .await?;
    state_pool.close().await;

    // Replace the logs database with one whose schema disagrees with its rows.
    let logs_path = logs_db_path(ody_home.as_path());
    let fixture = logs_path.with_extension("fixture");
    let fixture_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&fixture)
                .create_if_missing(true),
        )
        .await?;
    sqlx::raw_sql(
        "CREATE TABLE sample(value INTEGER);
         INSERT INTO sample VALUES (NULL);
         PRAGMA writable_schema=ON;
         UPDATE sqlite_schema SET sql='CREATE TABLE sample(value INTEGER NOT NULL)' WHERE name='sample';
         PRAGMA writable_schema=OFF;",
    )
    .execute(&fixture_pool)
    .await?;
    fixture_pool.close().await;
    tokio::fs::remove_file(&logs_path).await?;
    tokio::fs::rename(&fixture, &logs_path).await?;

    // Startup must detect the silent corruption, preserve the damaged file,
    // and continue with a fresh logs database.
    let telemetry = CorruptionTelemetry::default();
    let runtime = StateRuntime::init_with_telemetry_for_tests(
        ody_home.clone(),
        "test-provider".to_string(),
        &telemetry,
    )
    .await?;
    runtime.close().await;

    assert_eq!(
        telemetry.events(),
        vec![CorruptionEvent {
            db: "logs".to_string(),
        }]
    );

    let backups =
        std::fs::read_dir(ody_home.join("db-backups"))?.collect::<std::io::Result<Vec<_>>>()?;
    assert_eq!(backups.len(), 1, "corruption must create one backup folder");
    let backup_path = backups[0]
        .path()
        .join(logs_path.file_name().expect("logs filename"));
    let backup_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&backup_path)
                .create_if_missing(false)
                .read_only(true),
        )
        .await?;
    assert_eq!(
        sqlx::query_scalar::<_, String>("PRAGMA quick_check(1)")
            .fetch_one(&backup_pool)
            .await?,
        "NULL value in sample.value"
    );
    backup_pool.close().await;

    let state_pool = open_state_sqlite(
        &state_db_path(ody_home.as_path()),
        &runtime_state_migrator(),
        /*telemetry_override*/ None,
    )
    .await?;
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT value FROM preserved")
            .fetch_all(&state_pool)
            .await?,
        vec!["keep me".to_string()]
    );
    state_pool.close().await;
    let _ = tokio::fs::remove_dir_all(ody_home).await;
    Ok(())
}

#[tokio::test]
async fn quick_check_attempts_are_cached_per_owner() -> anyhow::Result<()> {
    let ody_home = unique_temp_dir();
    tokio::fs::create_dir_all(&ody_home).await?;
    let path = logs_db_path(ody_home.as_path());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await?;
    // Corrupt the schema root page so the check fails with a SQLite error
    // rather than a finding row.
    let mut connection = pool.acquire().await?;
    sqlx::raw_sql(
        "CREATE TABLE sample(value INTEGER);
         PRAGMA writable_schema=ON;
         UPDATE sqlite_schema SET rootpage=2147483647 WHERE name='sample';
         PRAGMA writable_schema=OFF;
         PRAGMA schema_version=99;",
    )
    .execute(&mut *connection)
    .await?;
    connection.return_to_pool().await;

    let validation = SqliteQuickCheckManager::new();
    assert_eq!(
        validation
            .quick_check_once(&pool, &path, Duration::from_secs(5))
            .await?,
        QuickCheckOutcome::CorruptedNeedsFixed
    );
    // Another owner must check the same file independently.
    let independent_validation = SqliteQuickCheckManager::new();
    assert_eq!(
        independent_validation
            .quick_check_once(&pool, &path, Duration::from_secs(5))
            .await?,
        QuickCheckOutcome::CorruptedNeedsFixed
    );
    pool.close().await;
    // A closed pool proves the completed attempt is skipped without another query.
    assert_eq!(
        validation
            .quick_check_once(&pool, &path, Duration::from_secs(5))
            .await?,
        QuickCheckOutcome::Skipped
    );
    let _ = tokio::fs::remove_dir_all(ody_home).await;
    Ok(())
}

#[tokio::test]
async fn locked_database_quick_check_is_incomplete_not_corruption() -> anyhow::Result<()> {
    let ody_home = unique_temp_dir();
    tokio::fs::create_dir_all(&ody_home).await?;
    let path = logs_db_path(ody_home.as_path());
    // A single-connection writer pool: converting WAL back to DELETE journal
    // mode fails immediately while any other idle WAL connection exists.
    let writer_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(Duration::from_secs(5)),
        )
        .await?;
    sqlx::query("CREATE TABLE sample(value INTEGER)")
        .execute(&writer_pool)
        .await?;
    let mut writer = writer_pool.acquire().await?;
    sqlx::query("PRAGMA journal_mode=DELETE")
        .execute(&mut *writer)
        .await?;
    let reader_pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(false)
                .read_only(true)
                .busy_timeout(Duration::from_millis(50)),
        )
        .await?;
    sqlx::query("BEGIN EXCLUSIVE").execute(&mut *writer).await?;

    let started = Instant::now();
    let result = quick_check(&reader_pool, started + Duration::from_millis(50)).await;
    sqlx::query("ROLLBACK").execute(&mut *writer).await?;
    drop(writer);
    writer_pool.close().await;

    assert_eq!(result?, QuickCheckOutcome::Incomplete);
    assert_eq!(
        quick_check(&reader_pool, Instant::now() + Duration::from_secs(5)).await?,
        QuickCheckOutcome::Complete
    );
    reader_pool.close().await;
    let _ = tokio::fs::remove_dir_all(ody_home).await;
    Ok(())
}

#[tokio::test]
async fn interrupted_quick_check_leaves_connection_usable() -> anyhow::Result<()> {
    let ody_home = unique_temp_dir();
    tokio::fs::create_dir_all(&ody_home).await?;
    let path = logs_db_path(ody_home.as_path());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await?;
    sqlx::raw_sql(
        "CREATE TABLE sample(value INTEGER);
         WITH RECURSIVE rows(id) AS (SELECT 1 UNION ALL SELECT id + 1 FROM rows WHERE id < 2048)
         INSERT INTO sample SELECT id FROM rows;",
    )
    .execute(&pool)
    .await?;

    // An already-expired budget interrupts the scan without corrupting state.
    assert_eq!(
        quick_check(&pool, Instant::now()).await?,
        QuickCheckOutcome::Incomplete
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT sum(value) FROM sample")
            .fetch_one(&pool)
            .await?,
        2_098_176
    );
    assert_eq!(
        quick_check(&pool, Instant::now() + Duration::from_secs(5)).await?,
        QuickCheckOutcome::Complete
    );
    pool.close().await;
    let _ = tokio::fs::remove_dir_all(ody_home).await;
    Ok(())
}
