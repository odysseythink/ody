//! TUI-owned Windows sandbox helpers retained while setup still runs in the local client process.
//!
//! TODO: These helpers inspect and modify the TUI host, so they do not support
//! cross-platform remote app servers. Move readiness and setup to the existing
//! `windowsSandbox/*` RPCs while preserving the pending permission profile,
//! use the server platform reported during initialization, and add a remote
//! equivalent for read-root grants.

use crate::legacy_core::config::Config;
use ody_config::types::WindowsSandboxModeToml;
use ody_features::Feature;
use ody_protocol::config_types::WindowsSandboxLevel;
// These types appear in the signatures of the non-Windows stub fns below too, so
// they must be imported on every target the module compiles for (it is built on
// non-Windows under `cfg(test)`; see `mod windows_sandbox` in lib.rs).
use ody_protocol::models::PermissionProfile;
use ody_utils_absolute_path::AbsolutePathBuf;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

pub(crate) fn level_from_config(config: &Config) -> WindowsSandboxLevel {
    match config.permissions.windows_sandbox_mode {
        Some(WindowsSandboxModeToml::Elevated) => WindowsSandboxLevel::Elevated,
        Some(WindowsSandboxModeToml::Unelevated) => WindowsSandboxLevel::RestrictedToken,
        None if config.features.enabled(Feature::WindowsSandboxElevated) => {
            WindowsSandboxLevel::Elevated
        }
        None if config.features.enabled(Feature::WindowsSandbox) => {
            WindowsSandboxLevel::RestrictedToken
        }
        None => WindowsSandboxLevel::Disabled,
    }
}

#[cfg(all(target_os = "windows", feature = "windows-sandbox"))]
pub(crate) use ody_windows_sandbox::sandbox_setup_is_complete;

#[cfg(all(target_os = "windows", not(feature = "windows-sandbox")))]
pub(crate) fn sandbox_setup_is_complete(_ody_home: &Path) -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn sandbox_setup_is_complete(_ody_home: &Path) -> bool {
    false
}

#[cfg(all(target_os = "windows", feature = "windows-sandbox"))]
pub(crate) fn run_elevated_setup(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    command_cwd: &Path,
    env_map: &HashMap<String, String>,
    ody_home: &Path,
) -> anyhow::Result<()> {
    let permissions = ody_windows_sandbox::ResolvedWindowsSandboxPermissions::try_from_permission_profile_for_workspace_roots(
        permission_profile,
        workspace_roots,
    )?;
    ody_windows_sandbox::run_elevated_setup(
        ody_windows_sandbox::SandboxSetupRequest {
            permissions: &permissions,
            command_cwd,
            env_map,
            ody_home,
            proxy_enforced: false,
        },
        ody_windows_sandbox::SetupRootOverrides::default(),
    )
}

#[cfg(all(target_os = "windows", not(feature = "windows-sandbox")))]
pub(crate) fn run_elevated_setup(
    _permission_profile: &PermissionProfile,
    _workspace_roots: &[AbsolutePathBuf],
    _command_cwd: &Path,
    _env_map: &HashMap<String, String>,
    _ody_home: &Path,
) -> anyhow::Result<()> {
    anyhow::bail!("elevated Windows sandbox setup is only available when the windows-sandbox feature is enabled")
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn run_elevated_setup(
    _permission_profile: &PermissionProfile,
    _workspace_roots: &[AbsolutePathBuf],
    _command_cwd: &Path,
    _env_map: &HashMap<String, String>,
    _ody_home: &Path,
) -> anyhow::Result<()> {
    anyhow::bail!("elevated Windows sandbox setup is only supported on Windows")
}

#[cfg(all(target_os = "windows", feature = "windows-sandbox"))]
pub(crate) fn elevated_setup_failure_details(err: &anyhow::Error) -> Option<(String, String)> {
    let failure = ody_windows_sandbox::extract_setup_failure(err)?;
    Some((
        failure.code.as_str().to_string(),
        ody_windows_sandbox::sanitize_setup_metric_tag_value(&failure.message),
    ))
}

#[cfg(all(target_os = "windows", not(feature = "windows-sandbox")))]
pub(crate) fn elevated_setup_failure_details(_err: &anyhow::Error) -> Option<(String, String)> {
    None
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn elevated_setup_failure_details(_err: &anyhow::Error) -> Option<(String, String)> {
    None
}

#[cfg(all(target_os = "windows", feature = "windows-sandbox"))]
pub(crate) fn elevated_setup_failure_metric_name(err: &anyhow::Error) -> &'static str {
    if ody_windows_sandbox::extract_setup_failure(err).is_some_and(|failure| {
        matches!(
            failure.code,
            ody_windows_sandbox::SetupErrorCode::OrchestratorHelperLaunchCanceled
        )
    }) {
        "ody.windows_sandbox.elevated_setup_canceled"
    } else {
        "ody.windows_sandbox.elevated_setup_failure"
    }
}

#[cfg(all(target_os = "windows", not(feature = "windows-sandbox")))]
pub(crate) fn elevated_setup_failure_metric_name(_err: &anyhow::Error) -> &'static str {
    panic!("elevated_setup_failure_metric_name is only supported when the windows-sandbox feature is enabled")
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn elevated_setup_failure_metric_name(_err: &anyhow::Error) -> &'static str {
    panic!("elevated_setup_failure_metric_name is only supported on Windows")
}

#[cfg(all(target_os = "windows", feature = "windows-sandbox"))]
pub(crate) fn grant_read_root_non_elevated(
    permission_profile: &PermissionProfile,
    workspace_roots: &[AbsolutePathBuf],
    command_cwd: &Path,
    env_map: &HashMap<String, String>,
    ody_home: &Path,
    read_root: &Path,
) -> anyhow::Result<PathBuf> {
    if !read_root.is_absolute() {
        anyhow::bail!("path must be absolute: {}", read_root.display());
    }
    if !read_root.exists() {
        anyhow::bail!("path does not exist: {}", read_root.display());
    }
    if !read_root.is_dir() {
        anyhow::bail!("path must be a directory: {}", read_root.display());
    }

    let canonical_root = dunce::canonicalize(read_root)?;
    ody_windows_sandbox::run_setup_refresh_with_extra_read_roots(
        permission_profile,
        workspace_roots,
        command_cwd,
        env_map,
        ody_home,
        vec![canonical_root.clone()],
        /*proxy_enforced*/ false,
    )?;
    Ok(canonical_root)
}

#[cfg(all(target_os = "windows", not(feature = "windows-sandbox")))]
pub(crate) fn grant_read_root_non_elevated(
    _permission_profile: &PermissionProfile,
    _workspace_roots: &[AbsolutePathBuf],
    _command_cwd: &Path,
    _env_map: &HashMap<String, String>,
    _ody_home: &Path,
    _read_root: &Path,
) -> anyhow::Result<PathBuf> {
    anyhow::bail!("Windows sandbox read-root grants are only available when the windows-sandbox feature is enabled")
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn grant_read_root_non_elevated(
    _permission_profile: &PermissionProfile,
    _workspace_roots: &[AbsolutePathBuf],
    _command_cwd: &Path,
    _env_map: &HashMap<String, String>,
    _ody_home: &Path,
    _read_root: &Path,
) -> anyhow::Result<PathBuf> {
    anyhow::bail!("Windows sandbox read-root grants are only supported on Windows")
}
