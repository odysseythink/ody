//! Per-machine agent configuration (TOML).

use ody_utils_absolute_path::AbsolutePathBuf;
use serde::Deserialize;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConfig {
    /// wss URL of the gateway's agent endpoint, e.g. `wss://host/ws/agent`.
    pub gateway_url: String,
    /// Shared secret presented as a Bearer token to the gateway.
    pub token: String,
    /// Machine name; instance ids are `<machine>/<name>`.
    pub machine: String,
    /// Delay between reconnect attempts (gateway or local app-server).
    pub reconnect_delay: std::time::Duration,
    /// Extra CA/certificate PEM to trust for the gateway TLS handshake, on
    /// top of the system roots (e.g. a Caddy internal-CA root cert).
    pub gateway_ca_cert: Option<AbsolutePathBuf>,
    pub instances: Vec<InstanceConf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceConf {
    pub name: String,
    pub url: String,
    /// Capability token for the local ody app-server, when it requires auth.
    pub token: Option<String>,
    machine: String,
}

impl InstanceConf {
    pub fn id(&self) -> String {
        format!("{}/{}", self.machine, self.name)
    }
}

#[derive(Deserialize)]
struct RawConfig {
    gateway_url: String,
    token: String,
    machine: String,
    #[serde(default = "default_reconnect_delay_ms")]
    reconnect_delay_ms: u64,
    gateway_ca_cert: Option<String>,
    #[serde(default)]
    instances: Vec<RawInstance>,
}

fn default_reconnect_delay_ms() -> u64 {
    5000
}

#[derive(Deserialize)]
struct RawInstance {
    name: String,
    url: String,
    token: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("{0}")]
    Invalid(String),
}

impl AgentConfig {
    pub fn from_toml_str(text: &str) -> Result<Self, ConfigError> {
        let raw: RawConfig = toml::from_str(text)?;
        if raw.machine.trim().is_empty() {
            return Err(ConfigError::Invalid("machine must not be empty".into()));
        }
        if raw.token.is_empty() {
            return Err(ConfigError::Invalid("token must not be empty".into()));
        }
        if !raw.gateway_url.starts_with("ws://") && !raw.gateway_url.starts_with("wss://") {
            return Err(ConfigError::Invalid(
                "gateway_url must be a ws:// or wss:// URL".into(),
            ));
        }
        if raw.instances.is_empty() {
            return Err(ConfigError::Invalid(
                "at least one [[instances]] entry is required".into(),
            ));
        }
        let gateway_ca_cert = raw
            .gateway_ca_cert
            .map(|path| {
                AbsolutePathBuf::from_absolute_path_checked(&path).map_err(|_| {
                    ConfigError::Invalid(format!(
                        "gateway_ca_cert must be an absolute path, got `{path}`"
                    ))
                })
            })
            .transpose()?;
        let mut names = HashSet::new();
        let mut instances = Vec::with_capacity(raw.instances.len());
        for inst in raw.instances {
            if inst.name.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "instance name must not be empty".into(),
                ));
            }
            if !names.insert(inst.name.clone()) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate instance name `{}`",
                    inst.name
                )));
            }
            if !inst.url.starts_with("ws://") && !inst.url.starts_with("wss://") {
                return Err(ConfigError::Invalid(format!(
                    "instance `{}` url must be ws:// or wss://",
                    inst.name
                )));
            }
            instances.push(InstanceConf {
                name: inst.name,
                url: inst.url,
                token: inst.token,
                machine: raw.machine.clone(),
            });
        }
        Ok(Self {
            gateway_url: raw.gateway_url,
            token: raw.token,
            machine: raw.machine,
            reconnect_delay: std::time::Duration::from_millis(raw.reconnect_delay_ms),
            gateway_ca_cert,
            instances,
        })
    }
}
