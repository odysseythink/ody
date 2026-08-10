//! Postgres provider stub.
use crate::config::DatabaseConnectionConfig;
use crate::error::DatabaseError;
use crate::provider::{DatabaseProvider, SharedDatabaseProvider};

#[derive(Debug)]
pub struct PostgresProvider;

impl PostgresProvider {
    pub fn new(_config: DatabaseConnectionConfig) -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl DatabaseProvider for PostgresProvider {
    fn name(&self) -> &str {
        "postgres"
    }

    async fn query(&self, _sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        Err(DatabaseError::Query(
            "Postgres provider not yet implemented".to_string(),
        ))
    }
}

#[derive(Debug)]
pub struct PostgresProviderFactory;

impl crate::provider::DatabaseProviderFactory for PostgresProviderFactory {
    fn name(&self) -> &str {
        "postgres"
    }

    fn create(&self, config: DatabaseConnectionConfig) -> Result<SharedDatabaseProvider, DatabaseError> {
        Ok(std::sync::Arc::new(PostgresProvider::new(config)))
    }
}
