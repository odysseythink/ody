//! Unified SQLx-backed database provider using the `Any` driver.
use crate::config::{DatabaseConnectionConfig, DatabaseProviderName};
use crate::error::DatabaseError;
use crate::provider::{DatabaseProvider, DatabaseQueryResult, SharedDatabaseProvider};
use sqlx::AnyPool;
use sqlx::{AssertSqlSafe, Column, Row};
use urlencoding::encode;

#[derive(Debug)]
pub struct SqlxProvider {
    name: String,
    pool: AnyPool,
}

impl SqlxProvider {
    pub async fn new(config: DatabaseConnectionConfig) -> Result<Self, DatabaseError> {
        // `AnyPool` does not install compiled database drivers automatically.
        // Without this call, its first connection attempt panics with
        // "No drivers installed" instead of returning a connection error.
        sqlx::any::install_default_drivers();
        let url = connection_url(&config);
        let pool = AnyPool::connect(&url)
            .await
            .map_err(|e| DatabaseError::Connection(format!("failed to connect to {url}: {e}")))?;
        Ok(Self {
            name: config.connection,
            pool,
        })
    }
}

#[async_trait::async_trait]
impl DatabaseProvider for SqlxProvider {
    fn name(&self) -> &str {
        &self.name
    }

    async fn query(&self, sql: &str) -> Result<DatabaseQueryResult, DatabaseError> {
        let sql = sql.to_string();
        let rows = sqlx::query(AssertSqlSafe(sql.as_str()))
            .fetch_all(&self.pool)
            .await
            .map_err(|e| DatabaseError::Query(format!("query failed: {e}")))?;

        if rows.is_empty() {
            return Ok(DatabaseQueryResult {
                columns: Vec::new(),
                rows: Vec::new(),
            });
        }

        let columns = rows[0]
            .columns()
            .iter()
            .map(|c| c.name().to_string())
            .collect();

        let mut result_rows = Vec::with_capacity(rows.len());
        for row in rows {
            let mut values = Vec::with_capacity(row.columns().len());
            for i in 0..row.columns().len() {
                let value: String = row
                    .try_get::<String, usize>(i)
                    .or_else(|_| row.try_get::<i64, usize>(i).map(|v| v.to_string()))
                    .or_else(|_| row.try_get::<f64, usize>(i).map(|v| v.to_string()))
                    .or_else(|_| row.try_get::<bool, usize>(i).map(|v| v.to_string()))
                    .or_else(|_| row.try_get::<Option<String>, usize>(i).map(|v| v.unwrap_or_default()))
                    .unwrap_or_else(|_| "<unsupported>".to_string());
                values.push(value);
            }
            result_rows.push(values);
        }

        Ok(DatabaseQueryResult {
            columns,
            rows: result_rows,
        })
    }
}

#[derive(Debug)]
pub struct SqlxProviderFactory;

impl crate::provider::DatabaseProviderFactory for SqlxProviderFactory {
    fn name(&self) -> &str {
        "sqlx"
    }

    fn create(&self, config: DatabaseConnectionConfig) -> Result<SharedDatabaseProvider, DatabaseError> {
        // SqlxProvider::new is async, so we cannot call it directly in a sync factory.
        // The factory is intended to be used from a context that can spawn the connection
        // on a runtime. For now, return a provider that defers connection creation.
        Ok(std::sync::Arc::new(DeferredSqlxProvider::new(config)))
    }
}

/// Provider that connects to the database on first query.
#[derive(Debug)]
pub struct DeferredSqlxProvider {
    config: DatabaseConnectionConfig,
    connected: tokio::sync::OnceCell<SqlxProvider>,
}

impl DeferredSqlxProvider {
    pub fn new(config: DatabaseConnectionConfig) -> Self {
        Self {
            config,
            connected: tokio::sync::OnceCell::const_new(),
        }
    }

    async fn get_provider(&self) -> Result<&SqlxProvider, DatabaseError> {
        self.connected
            .get_or_try_init(|| SqlxProvider::new(self.config.clone()))
            .await
    }
}

#[async_trait::async_trait]
impl DatabaseProvider for DeferredSqlxProvider {
    fn name(&self) -> &str {
        &self.config.connection
    }

    async fn query(&self, sql: &str) -> Result<DatabaseQueryResult, DatabaseError> {
        let provider = self.get_provider().await?;
        provider.query(sql).await
    }
}

fn connection_url(config: &DatabaseConnectionConfig) -> String {
    match config.provider {
        DatabaseProviderName::Postgres => {
            let password = config
                .password
                .as_ref()
                .map(|p| encode(p).to_string())
                .unwrap_or_default();
            format!(
                "postgres://{}:{}@{}:{}/{}",
                config.username, password, config.host, config.port, config.database
            )
        }
        DatabaseProviderName::Mysql => {
            let password = config
                .password
                .as_ref()
                .map(|p| encode(p).to_string())
                .unwrap_or_default();
            format!(
                "mysql://{}:{}@{}:{}/{}",
                config.username, password, config.host, config.port, config.database
            )
        }
        DatabaseProviderName::Sqlite => {
            format!("sqlite://{}", config.host)
        }
    }
}
