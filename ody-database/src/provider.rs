//! Shared database provider trait.
use crate::config::DatabaseConnectionConfig;
use crate::error::DatabaseError;

/// Result of executing a SQL query.
#[derive(Debug, Clone)]
pub struct DatabaseQueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[async_trait::async_trait]
pub trait DatabaseProvider: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;
    async fn query(&self, sql: &str) -> Result<DatabaseQueryResult, DatabaseError>;
}

pub type SharedDatabaseProvider = std::sync::Arc<dyn DatabaseProvider>;

pub trait DatabaseProviderFactory: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;
    fn create(&self, config: DatabaseConnectionConfig) -> Result<SharedDatabaseProvider, DatabaseError>;
}
