use std::collections::HashMap;

use crate::config::DatabaseConnectionConfig;
use crate::error::DatabaseError;
use crate::provider::{DatabaseProviderFactory, SharedDatabaseProvider};
use crate::providers::sqlx::SqlxProviderFactory;

pub struct DatabaseProviderRegistry {
    factories: HashMap<String, Box<dyn DatabaseProviderFactory>>,
}

impl DatabaseProviderRegistry {
    pub fn new() -> Self {
        let mut factories: HashMap<String, Box<dyn DatabaseProviderFactory>> = HashMap::new();
        factories.insert("sqlx".to_string(), Box::new(SqlxProviderFactory));
        Self { factories }
    }

    pub fn register(&mut self, factory: Box<dyn DatabaseProviderFactory>) {
        self.factories.insert(factory.name().to_string(), factory);
    }

    /// Create a provider for a single connection config.
    /// The provider name is derived from the database engine (Postgres/MySQL/SQLite) and is
    /// backed by SQLx's `Any` driver by default.
    pub fn create(
        &self,
        config: &DatabaseConnectionConfig,
    ) -> Result<SharedDatabaseProvider, DatabaseError> {
        let factory = self
            .factories
            .get("sqlx")
            .ok_or_else(|| DatabaseError::UnknownProvider(config.provider.to_string()))?;
        factory.create(config.clone())
    }

    /// Create providers for every configured connection in `database` and return them keyed by name.
    pub fn create_all(
        &self,
        database: &crate::config::DatabaseConfig,
    ) -> Result<HashMap<String, SharedDatabaseProvider>, DatabaseError> {
        database
            .connections
            .values()
            .map(|config| self.create(config).map(|provider| (config.connection.clone(), provider)))
            .collect()
    }
}

impl Default for DatabaseProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DatabaseConnectionConfig, DatabaseProviderName};

    fn sqlite_config() -> DatabaseConnectionConfig {
        DatabaseConnectionConfig {
            connection: "test".to_string(),
            provider: DatabaseProviderName::Sqlite,
            host: ":memory:".to_string(),
            port: 0,
            database: "".to_string(),
            username: "".to_string(),
            password: None,
            options: HashMap::new(),
        }
    }

    #[test]
    fn default_registry_creates_provider() {
        let registry = DatabaseProviderRegistry::new();
        let config = sqlite_config();
        let result = registry.create(&config);
        assert!(result.is_ok(), "expected registered provider to be created: {:?}", result);
    }

    #[test]
    fn create_all_returns_providers_for_every_connection() {
        let mut connections = HashMap::new();
        connections.insert(
            "a".to_string(),
            DatabaseConnectionConfig {
                connection: "a".to_string(),
                provider: DatabaseProviderName::Sqlite,
                host: ":memory:".to_string(),
                port: 0,
                database: "".to_string(),
                username: "".to_string(),
                password: None,
                options: HashMap::new(),
            },
        );
        connections.insert(
            "b".to_string(),
            DatabaseConnectionConfig {
                connection: "b".to_string(),
                provider: DatabaseProviderName::Sqlite,
                host: ":memory:".to_string(),
                port: 0,
                database: "".to_string(),
                username: "".to_string(),
                password: None,
                options: HashMap::new(),
            },
        );
        let database = crate::config::DatabaseConfig {
            primary: "a".to_string(),
            connections,
        };
        let registry = DatabaseProviderRegistry::new();
        let providers = registry.create_all(&database).expect("create all providers");
        assert_eq!(providers.len(), 2);
        assert!(providers.contains_key("a"));
        assert!(providers.contains_key("b"));
    }
}
