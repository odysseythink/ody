//! MySQL provider stub.
use crate::config::DatabaseConnectionConfig;
use crate::error::DatabaseError;
use crate::provider::{DatabaseProvider, SharedDatabaseProvider};

#[derive(Debug)]
pub struct MysqlProvider;

impl MysqlProvider {
    pub fn new(_config: DatabaseConnectionConfig) -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl DatabaseProvider for MysqlProvider {
    fn name(&self) -> &str {
        "mysql"
    }

    async fn query(&self, _sql: &str) -> Result<Vec<Vec<String>>, DatabaseError> {
        Err(DatabaseError::Query(
            "MySQL provider not yet implemented".to_string(),
        ))
    }
}

#[derive(Debug)]
pub struct MysqlProviderFactory;

impl crate::provider::DatabaseProviderFactory for MysqlProviderFactory {
    fn name(&self) -> &str {
        "mysql"
    }

    fn create(&self, config: DatabaseConnectionConfig) -> Result<SharedDatabaseProvider, DatabaseError> {
        Ok(std::sync::Arc::new(MysqlProvider::new(config)))
    }
}
