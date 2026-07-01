//! Ody Apps support for the host-owned apps MCP server.
//!
//! This module owns the pieces that are unique to ChatGPT-hosted app
//! connectors: cache scoping by authenticated user, disk cache reads/writes,
//! connector allow-list filtering, and the normalization that turns app
//! connector/tool metadata into model-visible MCP callable names.

use std::path::PathBuf;
use std::time::Instant;

use crate::mcp::ODY_APPS_MCP_SERVER_NAME;
use crate::runtime::emit_duration;
use crate::tools::MCP_TOOLS_CACHE_WRITE_DURATION_METRIC;
use crate::tools::ToolInfo;
use anyhow::Context;
use ody_login::OdyAuth;
use ody_protocol::mcp::McpServerInfo;
use ody_utils_plugins::mcp_connector::sanitize_name;
use serde::Deserialize;
use serde::Serialize;
use sha1::Digest;
use sha1::Sha1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OdyAppsToolsCacheKey {
    pub(crate) account_id: Option<String>,
    pub(crate) chatgpt_user_id: Option<String>,
    pub(crate) is_workspace_account: bool,
}

pub fn ody_apps_tools_cache_key(auth: Option<&OdyAuth>) -> OdyAppsToolsCacheKey {
    OdyAppsToolsCacheKey {
        account_id: auth.and_then(OdyAuth::get_account_id),
        chatgpt_user_id: auth.and_then(OdyAuth::get_chatgpt_user_id),
        is_workspace_account: auth.is_some_and(OdyAuth::is_workspace_account),
    }
}

#[derive(Clone)]
pub(crate) struct OdyAppsToolsCacheContext {
    pub(crate) ody_home: PathBuf,
    pub(crate) user_key: OdyAppsToolsCacheKey,
}

impl OdyAppsToolsCacheContext {
    pub(crate) fn tools_cache_path(&self) -> PathBuf {
        self.cache_path_in(ODY_APPS_TOOLS_CACHE_DIR)
    }

    pub(crate) fn server_info_cache_path(&self) -> PathBuf {
        self.cache_path_in(ODY_APPS_SERVER_INFO_CACHE_DIR)
    }

    fn cache_path_in(&self, cache_dir: &str) -> PathBuf {
        let user_key_json = serde_json::to_string(&self.user_key).unwrap_or_default();
        let user_key_hash = sha1_hex(&user_key_json);
        self.ody_home
            .join(cache_dir)
            .join(format!("{user_key_hash}.json"))
    }
}

pub(crate) enum CachedOdyAppsToolsLoad {
    Hit(Vec<ToolInfo>),
    Missing,
    Invalid,
}

pub(crate) fn normalize_ody_apps_tool_title(
    server_name: &str,
    connector_name: Option<&str>,
    value: &str,
) -> String {
    if server_name != ODY_APPS_MCP_SERVER_NAME {
        return value.to_string();
    }

    let Some(connector_name) = connector_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
    else {
        return value.to_string();
    };

    let prefix = format!("{connector_name}_");
    if let Some(stripped) = value.strip_prefix(&prefix)
        && !stripped.is_empty()
    {
        return stripped.to_string();
    }

    value.to_string()
}

pub(crate) fn normalize_ody_apps_callable_name(
    server_name: &str,
    tool_name: &str,
    connector_id: Option<&str>,
    connector_name: Option<&str>,
) -> String {
    if server_name != ODY_APPS_MCP_SERVER_NAME {
        return tool_name.to_string();
    }

    let tool_name = sanitize_name(tool_name);

    if let Some(connector_name) = connector_name
        .map(str::trim)
        .map(sanitize_name)
        .filter(|name| !name.is_empty())
        && let Some(stripped) = tool_name.strip_prefix(&connector_name)
        && !stripped.is_empty()
    {
        return stripped.to_string();
    }

    if let Some(connector_id) = connector_id
        .map(str::trim)
        .map(sanitize_name)
        .filter(|name| !name.is_empty())
        && let Some(stripped) = tool_name.strip_prefix(&connector_id)
        && !stripped.is_empty()
    {
        return stripped.to_string();
    }

    tool_name
}

pub(crate) fn normalize_ody_apps_callable_namespace(
    server_name: &str,
    connector_name: Option<&str>,
) -> String {
    if server_name == ODY_APPS_MCP_SERVER_NAME
        && let Some(connector_name) = connector_name
    {
        format!("{}__{}", server_name, sanitize_name(connector_name))
    } else {
        server_name.to_string()
    }
}

pub(crate) fn write_cached_ody_apps_tools_if_needed(
    server_name: &str,
    cache_context: Option<&OdyAppsToolsCacheContext>,
    server_info: &McpServerInfo,
    tools: &[ToolInfo],
) {
    if server_name != ODY_APPS_MCP_SERVER_NAME {
        return;
    }

    if let Some(cache_context) = cache_context {
        let cache_write_start = Instant::now();
        write_cached_ody_apps_tools(cache_context, tools);
        if let Err(err) = write_cached_ody_apps_server_info(cache_context, server_info) {
            tracing::warn!("failed to write Ody Apps server info cache: {err:#}");
        }
        emit_duration(
            MCP_TOOLS_CACHE_WRITE_DURATION_METRIC,
            cache_write_start.elapsed(),
            &[],
        );
    }
}

pub(crate) fn load_startup_cached_ody_apps_tools_snapshot(
    server_name: &str,
    cache_context: Option<&OdyAppsToolsCacheContext>,
) -> Option<Vec<ToolInfo>> {
    if server_name != ODY_APPS_MCP_SERVER_NAME {
        return None;
    }

    let cache_context = cache_context?;

    match load_cached_ody_apps_tools(cache_context) {
        CachedOdyAppsToolsLoad::Hit(tools) => Some(tools),
        CachedOdyAppsToolsLoad::Missing | CachedOdyAppsToolsLoad::Invalid => None,
    }
}

pub(crate) fn load_startup_cached_ody_apps_server_info(
    server_name: &str,
    cache_context: Option<&OdyAppsToolsCacheContext>,
) -> Option<McpServerInfo> {
    if server_name != ODY_APPS_MCP_SERVER_NAME {
        return None;
    }

    load_cached_ody_apps_server_info(cache_context?)
}

#[cfg(test)]
pub(crate) fn read_cached_ody_apps_tools(
    cache_context: &OdyAppsToolsCacheContext,
) -> Option<Vec<ToolInfo>> {
    match load_cached_ody_apps_tools(cache_context) {
        CachedOdyAppsToolsLoad::Hit(tools) => Some(tools),
        CachedOdyAppsToolsLoad::Missing | CachedOdyAppsToolsLoad::Invalid => None,
    }
}

pub(crate) fn load_cached_ody_apps_tools(
    cache_context: &OdyAppsToolsCacheContext,
) -> CachedOdyAppsToolsLoad {
    let cache_path = cache_context.tools_cache_path();
    let bytes = match std::fs::read(cache_path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return CachedOdyAppsToolsLoad::Missing;
        }
        Err(_) => return CachedOdyAppsToolsLoad::Invalid,
    };
    let cache: OdyAppsToolsDiskCache = match serde_json::from_slice(&bytes) {
        Ok(cache) => cache,
        Err(_) => return CachedOdyAppsToolsLoad::Invalid,
    };
    if cache.schema_version != ODY_APPS_TOOLS_CACHE_SCHEMA_VERSION {
        return CachedOdyAppsToolsLoad::Invalid;
    }
    CachedOdyAppsToolsLoad::Hit(cache.tools)
}

pub(crate) fn write_cached_ody_apps_tools(
    cache_context: &OdyAppsToolsCacheContext,
    tools: &[ToolInfo],
) {
    let cache_path = cache_context.tools_cache_path();
    if let Some(parent) = cache_path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    let Ok(bytes) = serde_json::to_vec_pretty(&OdyAppsToolsDiskCache {
        schema_version: ODY_APPS_TOOLS_CACHE_SCHEMA_VERSION,
        tools: tools.to_vec(),
    }) else {
        return;
    };
    let _ = std::fs::write(cache_path, bytes);
}

pub(crate) fn load_cached_ody_apps_server_info(
    cache_context: &OdyAppsToolsCacheContext,
) -> Option<McpServerInfo> {
    let bytes = std::fs::read(cache_context.server_info_cache_path()).ok()?;
    let cache: OdyAppsServerInfoDiskCache = serde_json::from_slice(&bytes).ok()?;
    (cache.schema_version == ODY_APPS_SERVER_INFO_CACHE_SCHEMA_VERSION)
        .then_some(cache.server_info)
}

fn write_cached_ody_apps_server_info(
    cache_context: &OdyAppsToolsCacheContext,
    server_info: &McpServerInfo,
) -> anyhow::Result<()> {
    let cache_path = cache_context.server_info_cache_path();
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create Ody Apps server info cache directory `{}`",
                parent.display()
            )
        })?;
    }
    let bytes = serde_json::to_vec_pretty(&OdyAppsServerInfoDiskCache {
        schema_version: ODY_APPS_SERVER_INFO_CACHE_SCHEMA_VERSION,
        server_info: server_info.clone(),
    })
    .context("failed to serialize Ody Apps server info cache")?;
    std::fs::write(&cache_path, bytes).with_context(|| {
        format!(
            "failed to write Ody Apps server info cache `{}`",
            cache_path.display()
        )
    })?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OdyAppsToolsDiskCache {
    schema_version: u8,
    tools: Vec<ToolInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OdyAppsServerInfoDiskCache {
    schema_version: u8,
    server_info: McpServerInfo,
}

const ODY_APPS_TOOLS_CACHE_DIR: &str = "cache/ody_apps_tools";
pub(crate) const ODY_APPS_TOOLS_CACHE_SCHEMA_VERSION: u8 = 4;

const ODY_APPS_SERVER_INFO_CACHE_DIR: &str = "cache/ody_apps_server_info";
const ODY_APPS_SERVER_INFO_CACHE_SCHEMA_VERSION: u8 = 1;

fn sha1_hex(s: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(s.as_bytes());
    let sha1 = hasher.finalize();
    format!("{sha1:x}")
}
