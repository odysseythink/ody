pub mod auth;
pub mod auth_env_telemetry;

mod outbound_proxy;

pub use ody_client::BuildCustomCaTransportError as BuildLoginHttpClientError;
pub use ody_config::types::AuthCredentialsStoreMode;

pub use auth::AuthConfig;
pub use auth::AuthDotJson;
pub use auth::AuthKeyringBackendKind;
pub use auth::AuthManager;
pub use auth::AuthManagerConfig;
pub use auth::ODY_API_KEY_ENV_VAR;
pub use auth::OdyAuth;
pub use auth::OPENAI_API_KEY_ENV_VAR;
pub use auth::default_client;
pub use auth::enforce_login_restrictions;
pub use auth::load_auth_dot_json;
pub use auth::login_with_api_key;
pub use auth::logout;
pub use auth::read_odysseythink_api_key_from_env;
pub use auth::save_auth;
pub use auth_env_telemetry::AuthEnvTelemetry;
pub use auth_env_telemetry::collect_auth_env_telemetry;
pub use outbound_proxy::AuthRouteConfig;
