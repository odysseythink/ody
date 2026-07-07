use crate::config::Config;

/// Cache for per-workspace settings that used to be fetched from a remote workspace
/// service. That service has been removed, so the cache is now a no-op placeholder.
#[derive(Debug, Default)]
pub struct WorkspaceSettingsCache;

/// Workspace plugins enablement used to depend on remote workspace auth, which has
/// been removed. Ody plugins are enabled by default for all workspaces.
pub async fn ody_plugins_enabled_for_workspace(
    _config: &Config,
    _cache: Option<&WorkspaceSettingsCache>,
) -> anyhow::Result<bool> {
    Ok(true)
}
