use std::collections::HashMap;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use strum_macros::{Display, EnumString};

/// Supported database engines for Ody's built-in query tool.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    Display,
    EnumString,
    JsonSchema,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum DatabaseProviderName {
    Postgres,
    Mysql,
    Sqlite,
}

/// Per-connection configuration stored under `[services.database.connections.<name>]``.
#[derive(Clone, Deserialize, Serialize, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct DatabaseConnectionConfig {
    /// The connection preset name. Must match the table key it is stored under.
    pub connection: String,
    /// Database engine to connect to.
    pub provider: DatabaseProviderName,
    /// Hostname or IP address of the database server.
    /// For `sqlite` this is the path to the database file.
    pub host: String,
    /// Port the database server listens on.
    /// For `sqlite` this is ignored.
    pub port: u16,
    /// Database/schema name to connect to.
    /// For `sqlite` this is ignored.
    pub database: String,
    /// Username for authentication.
    /// For `sqlite` this is ignored.
    pub username: String,
    /// Optional password for authentication. If omitted, the provider may fall
    /// back to an environment variable or OS keyring.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Additional provider-specific options such as `sslmode` or `socket`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub options: HashMap<String, Value>,
}

impl fmt::Debug for DatabaseConnectionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatabaseConnectionConfig")
            .field("connection", &self.connection)
            .field("provider", &self.provider)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("options", &format!("{} entries", self.options.len()))
            .finish()
    }
}

/// Top-level `[services.database]` table in `config.toml`.
#[derive(Clone, Serialize, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct DatabaseConfig {
    /// The currently active primary connection preset.
    pub primary: String,
    /// Per-connection presets. The active connection's config is resolved from
    /// this map, falling back to an empty default if no preset has been saved.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub connections: HashMap<String, DatabaseConnectionConfig>,
}

impl DatabaseConfig {
    /// Resolve the full configuration for the currently active primary connection.
    pub fn primary_config(&self) -> Option<DatabaseConnectionConfig> {
        self.connections.get(&self.primary).cloned()
    }

    /// Return the saved preset for a connection, if any.
    pub fn connection_config(&self, name: &str) -> Option<&DatabaseConnectionConfig> {
        self.connections.get(name)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DatabaseConfigRepr {
    New {
        primary: String,
        #[serde(default)]
        connections: HashMap<String, DatabaseConnectionConfig>,
    },
    Legacy {
        primary: DatabaseConnectionConfig,
    },
}

impl<'de> Deserialize<'de> for DatabaseConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match DatabaseConfigRepr::deserialize(deserializer)? {
            DatabaseConfigRepr::New { primary, connections } => Ok(DatabaseConfig {
                primary,
                connections,
            }),
            DatabaseConfigRepr::Legacy { primary } => {
                let name = primary.connection.clone();
                let mut connections = HashMap::new();
                connections.insert(name.clone(), primary);
                Ok(DatabaseConfig {
                    primary: name,
                    connections,
                })
            }
        }
    }
}

impl fmt::Debug for DatabaseConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatabaseConfig")
            .field("primary", &self.primary)
            .field("connections", &format!("{} entries", self.connections.len()))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_config_masks_password_in_debug() {
        let config: DatabaseConfig = toml::from_str(
            r#"
primary = "db_connection_1"

[connections.db_connection_1]
connection = "db_connection_1"
provider = "postgres"
host = "127.0.0.1"
port = 5432
database = "project_a"
username = "ranwei"
password = "secret"

[connections.db_connection_1.options]
sslmode = "disable"
"#,
        )
        .expect("deserialize database config");

        assert_eq!(config.primary, "db_connection_1");
        let primary = config.primary_config().expect("primary preset");
        assert_eq!(primary.provider, DatabaseProviderName::Postgres);
        assert_eq!(primary.password.as_deref(), Some("secret"));

        let debug = format!("{:?}", primary);
        assert!(debug.contains("***"));
        assert!(!debug.contains("secret"));
    }

    #[test]
    fn new_format_database_config_deserializes_from_toml() {
        let config: DatabaseConfig = toml::from_str(
            r#"
primary = "db_connection_1"

[connections.db_connection_1]
connection = "db_connection_1"
provider = "postgres"
host = "127.0.0.1"
port = 5432
database = "mydb"
username = "user"

[connections.db_connection_1.options]
sslmode = "disable"
"#,
        )
        .expect("deserialize new format");

        assert_eq!(config.primary, "db_connection_1");
        let conn = config
            .connection_config("db_connection_1")
            .expect("db_connection_1 preset");
        assert_eq!(conn.provider, DatabaseProviderName::Postgres);
        assert_eq!(conn.port, 5432);
        assert_eq!(
            conn.options.get("sslmode").and_then(|v| v.as_str()),
            Some("disable")
        );
    }

    #[test]
    fn legacy_single_connection_format_deserializes_from_toml() {
        let config: DatabaseConfig = toml::from_str(
            r#"
primary = { connection = "legacy_db", provider = "sqlite", host = "/tmp/legacy.db", port = 0, database = "", username = "" }
"#,
        )
        .expect("deserialize legacy format");

        assert_eq!(config.primary, "legacy_db");
        assert!(config.connection_config("legacy_db").is_some());
    }

    #[test]
    fn services_config_with_database_round_trips() {
        let config = DatabaseConfig {
            primary: "db_connection_1".to_string(),
            connections: {
                let mut map = HashMap::new();
                map.insert(
                    "db_connection_1".to_string(),
                    DatabaseConnectionConfig {
                        connection: "db_connection_1".to_string(),
                        provider: DatabaseProviderName::Mysql,
                        host: "127.0.0.1".to_string(),
                        port: 3306,
                        database: "mydb".to_string(),
                        username: "root".to_string(),
                        password: None,
                        options: HashMap::new(),
                    },
                );
                map
            },
        };
        let json = serde_json::to_value(&config).expect("serialize database config");
        let back: DatabaseConfig = serde_json::from_value(json).expect("deserialize database config");
        assert_eq!(back, config);
    }
}
