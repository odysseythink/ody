use std::collections::HashMap;
use std::sync::Arc;

use ody_core::config::Config;
use ody_database::provider::SharedDatabaseProvider;
use ody_database::registry::DatabaseProviderRegistry;
use ody_database::tool::DatabaseQueryTool;
use ody_extension_api::{
    ConfigContributor, ExtensionData, ExtensionFuture, ExtensionRegistryBuilder,
    ThreadLifecycleContributor, ThreadStartInput, ToolCall, ToolContributor,
};
use ody_tools::ToolExecutor;

/// Per-thread handle holding all configured database connection providers and
/// the name of the primary connection preset.
#[derive(Clone)]
struct DatabaseConnectionsHandle {
    primary: String,
    connections: HashMap<String, SharedDatabaseProvider>,
}

/// App-server extension that wires the `DatabaseQuery` tool into a thread when
/// `[services.database]` is configured.
#[derive(Clone)]
pub struct DatabaseExtension;

impl DatabaseExtension {
    fn create_handle(services: &ody_web_search::config::ServicesConfig) -> Option<DatabaseConnectionsHandle> {
        let database_config = services.database.as_ref()?;
        let registry = DatabaseProviderRegistry::new();
        let connections = match registry.create_all(database_config) {
            Ok(connections) => connections,
            Err(err) => {
                tracing::warn!(error = %err, "failed to create database providers");
                return None;
            }
        };
        Some(DatabaseConnectionsHandle {
            primary: database_config.primary.clone(),
            connections,
        })
    }
}

impl ThreadLifecycleContributor<Config> for DatabaseExtension {
    fn on_thread_start<'a>(
        &'a self,
        input: ThreadStartInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if let Some(services) = input.config.services.as_ref() {
                if let Some(handle) = Self::create_handle(services) {
                    input.thread_store.insert(handle);
                }
            }
        })
    }
}

impl ConfigContributor<Config> for DatabaseExtension {
    fn on_config_changed(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        _previous_config: &Config,
        new_config: &Config,
    ) {
        if let Some(services) = new_config.services.as_ref() {
            if let Some(handle) = Self::create_handle(services) {
                thread_store.insert(handle);
            }
        } else {
            let _: Option<Arc<DatabaseConnectionsHandle>> = thread_store.remove();
        }
    }
}

impl ToolContributor for DatabaseExtension {
    fn tools(
        &self,
        session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
        let Some(handle) = thread_store.get::<DatabaseConnectionsHandle>() else {
            return Vec::new();
        };
        vec![Arc::new(DatabaseQueryTool::new(
            session_store.level_id().to_string(),
            handle.primary.clone(),
            handle.connections.clone(),
        ))]
    }
}

pub fn install(registry: &mut ExtensionRegistryBuilder<Config>) {
    let extension = Arc::new(DatabaseExtension);
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.tool_contributor(extension);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_database::config::{DatabaseConfig, DatabaseConnectionConfig, DatabaseProviderName};
    use ody_extension_api::ExtensionDataInit;

    fn services_config() -> ody_web_search::config::ServicesConfig {
        ody_web_search::config::ServicesConfig {
            web_search: None,
            browser: None,
            database: Some(DatabaseConfig {
                primary: "test".to_string(),
                connections: {
                    let mut map = HashMap::new();
                    map.insert(
                        "test".to_string(),
                        DatabaseConnectionConfig {
                            connection: "test".to_string(),
                            provider: DatabaseProviderName::Sqlite,
                            host: ":memory:".to_string(),
                            port: 0,
                            database: "".to_string(),
                            username: "".to_string(),
                            password: None,
                            options: HashMap::new(),
                        },
                    );
                    map
                },
            }),
        }
    }

    #[test]
    fn create_handle_returns_handle_for_configured_sqlite() {
        let services = services_config();
        assert!(DatabaseExtension::create_handle(&services).is_some());
    }

    #[test]
    fn create_handle_returns_none_when_database_config_missing() {
        let services = ody_web_search::config::ServicesConfig {
            web_search: None,
            browser: None,
            database: None,
        };
        assert!(DatabaseExtension::create_handle(&services).is_none());
    }

    #[test]
    fn tools_returns_empty_when_no_handle_in_thread_store() {
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("thread");
        let extension = DatabaseExtension;
        let tools = extension.tools(&session_store, &thread_store);
        assert!(tools.is_empty());
    }

    #[test]
    fn tools_returns_database_query_when_handle_present() {
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new_with_init("thread", ExtensionDataInit::new());
        let provider = DatabaseExtension::create_handle(&services_config())
            .expect("should create handle");
        thread_store.insert(provider);
        let extension = DatabaseExtension;
        let tools = extension.tools(&session_store, &thread_store);
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].tool_name(),
            ody_tools::ToolName::plain("DatabaseQuery")
        );
    }
}
