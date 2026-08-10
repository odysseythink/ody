//! SQLite provider stub.
use crate::config::DatabaseConnectionConfig;
use crate::error::DatabaseError;
use crate::provider::{DatabaseProvider, SharedDatabaseProvider};

#[derive(Debug)]
pub struct SqliteProvider;

impl SqliteProvider {
    pub fn new(_config: DatabaseConnectionConfig) -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl DatabaseProvider for SqliteProvider {
    fn name(&self) -> &str {
        "sqlite"
    }

    async fn query(&self, _sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        Err(DatabaseError::Query(
            "SQLite provider not yet implemented".to_string(),
        ))
    }
}

#[derive(Debug)]
pub struct SqliteProviderFactory;

impl crate::provider::DatabaseProviderFactory for SqliteProviderFactory {
    fn name(&self) -> &str {
        "sqlite"
    }

    fn create(&self, config: DatabaseConnectionConfig) -> Result<SharedDatabaseProvider, DatabaseError> {
        Ok(std::sync::Arc::new(SqliteProvider::new(config)))
    }
}
