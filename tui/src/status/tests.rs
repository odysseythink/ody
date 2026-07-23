use super::new_status_output;
use super::new_status_output_with_handle;
use crate::history_cell::HistoryCell;
use crate::legacy_core::config::Config;
use crate::legacy_core::config::ConfigBuilder;
use crate::legacy_core::config::PermissionProfileSnapshot;
use crate::status::remote_connection::RemoteConnectionStatus;
use crate::test_support::PathBufExt;
use crate::test_support::test_path_buf;
use crate::token_usage::TokenUsage;
use crate::token_usage::TokenUsageInfo;
use chrono::Duration as ChronoDuration;
use chrono::Local;
use chrono::TimeZone;
use chrono::Utc;
use insta::assert_snapshot;
use ody_app_server_protocol::AskForApproval;
use ody_config::LoaderOverrides;
use ody_models_manager::test_support::construct_model_info_offline_for_tests;
use ody_models_manager::test_support::get_model_offline_for_tests;
use ody_protocol::ThreadId;
use ody_protocol::config_types::ApprovalsReviewer;
use ody_protocol::config_types::ReasoningSummary;
use ody_protocol::model_metadata::ReasoningEffort;
use ody_protocol::models::ActivePermissionProfile;
use ody_protocol::models::BUILT_IN_PERMISSION_PROFILE_WORKSPACE;
use ody_protocol::models::ManagedFileSystemPermissions;
use ody_protocol::models::PermissionProfile;
use ody_protocol::permissions::FileSystemAccessMode;
use ody_protocol::permissions::FileSystemPath;
use ody_protocol::permissions::FileSystemSandboxEntry;
use ody_protocol::permissions::FileSystemSpecialPath;
use ody_protocol::permissions::NetworkSandboxPolicy;
use ody_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use ratatui::prelude::*;
use tempfile::TempDir;
use unicode_width::UnicodeWidthStr;

fn app_server_workspace_write_profile(network_enabled: bool) -> PermissionProfile {
    PermissionProfile::Managed {
        network: if network_enabled {
            NetworkSandboxPolicy::Enabled
        } else {
            NetworkSandboxPolicy::Restricted
        },
        file_system: ManagedFileSystemPermissions::Restricted {
            entries: vec![
                FileSystemSandboxEntry {
                    path: FileSystemPath::Special {
                        value: FileSystemSpecialPath::Root,
                    },
                    access: FileSystemAccessMode::Read,
                },
                FileSystemSandboxEntry {
                    path: FileSystemPath::Special {
                        value: FileSystemSpecialPath::ProjectRoots { subpath: None },
                    },
                    access: FileSystemAccessMode::Write,
                },
                FileSystemSandboxEntry {
                    path: FileSystemPath::Special {
                        value: FileSystemSpecialPath::SlashTmp,
                    },
                    access: FileSystemAccessMode::Write,
                },
                FileSystemSandboxEntry {
                    path: FileSystemPath::Special {
                        value: FileSystemSpecialPath::Tmpdir,
                    },
                    access: FileSystemAccessMode::Write,
                },
            ],
            glob_scan_max_depth: None,
        },
    }
}

async fn test_config(temp_home: &TempDir) -> Config {
    let mut config = ConfigBuilder::default()
        .ody_home(temp_home.path().to_path_buf())
        .loader_overrides(LoaderOverrides::without_managed_config_for_tests())
        .build()
        .await
        .expect("load config");
    config.approvals_reviewer = ApprovalsReviewer::User;
    config
        .permissions
        .set_permission_profile(app_server_workspace_write_profile(
            /*network_enabled*/ true,
        ))
        .expect("set permission profile");
    config.model_context_window = Some(272_000);
    config
}

fn set_workspace_cwd(config: &mut Config, cwd: AbsolutePathBuf) {
    config.cwd = cwd.clone();
    config.workspace_roots = vec![cwd];
    config
        .permissions
        .set_workspace_roots(config.workspace_roots.clone());
}

fn token_info_for(model_slug: &str, config: &Config, usage: &TokenUsage) -> TokenUsageInfo {
    let context_window =
        construct_model_info_offline_for_tests(model_slug, &config.to_models_manager_config())
            .context_window;
    TokenUsageInfo {
        total_token_usage: usage.clone(),
        last_token_usage: usage.clone(),
        model_context_window: context_window,
        auto_compact_token_limit: None,
    }
}

fn render_lines(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn sanitize_directory(lines: Vec<String>) -> Vec<String> {
    let frame_width = lines
        .iter()
        .find(|line| line.starts_with('╭'))
        .map(|line| UnicodeWidthStr::width(line.as_str()));
    lines
        .into_iter()
        .map(|line| {
            if let (Some(frame_width), Some(dir_pos), Some(pipe_idx)) =
                (frame_width, line.find("Directory: "), line.rfind('│'))
            {
                let prefix = &line[..dir_pos + "Directory: ".len()];
                let suffix = &line[pipe_idx..];
                let replacement = "[[workspace]]";
                let content_width = frame_width.saturating_sub(
                    UnicodeWidthStr::width(prefix) + UnicodeWidthStr::width(suffix),
                );
                let mut rebuilt = prefix.to_string();
                rebuilt.push_str(replacement);
                let replacement_width = UnicodeWidthStr::width(replacement);
                if content_width > replacement_width {
                    rebuilt.push_str(&" ".repeat(content_width - replacement_width));
                }
                rebuilt.push_str(suffix);
                rebuilt
            } else {
                line
            }
        })
        .collect()
}

fn reset_at_from(captured_at: &chrono::DateTime<chrono::Local>, seconds: i64) -> i64 {
    (*captured_at + ChronoDuration::seconds(seconds))
        .with_timezone(&Utc)
        .timestamp()
}

fn permissions_text_for(config: &Config) -> Option<String> {
    let usage = TokenUsage::default();
    let captured_at = chrono::Local
        .with_ymd_and_hms(2024, 1, 2, 3, 4, 5)
        .single()
        .expect("timestamp");
    let model_slug = get_model_offline_for_tests(config.model.as_deref());
    let composite = new_status_output(
        config,
        /*token_info*/ None,
        &usage,
        &None,
        /*thread_name*/ None,
        /*forked_from*/ None,
        captured_at,
        &model_slug,
        /*collaboration_mode*/ None,
        /*reasoning_effort_override*/ None,
    );
    render_lines(&composite.display_lines(/*width*/ 80))
        .iter()
        .find(|line| line.contains("Permissions:"))
        .and_then(|line| {
            line.split("Permissions:")
                .nth(1)
                .map(str::trim)
                .map(|text| text.trim_end_matches('│'))
                .map(str::trim)
                .map(ToString::to_string)
        })
}

#[tokio::test]
async fn status_snapshot_includes_reasoning_details() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config.model = Some("kimi-for-coding".to_string());
    config.model_provider_id = "kimi".to_string();
    config.model_reasoning_summary = Some(ReasoningSummary::Detailed);
    set_workspace_cwd(&mut config, test_path_buf("/workspace/tests").abs());
    config
        .permissions
        .set_permission_profile(PermissionProfile::workspace_write())
        .expect("set permission profile");

    let usage = TokenUsage {
        input_tokens: 1_200,
        cached_input_tokens: 200,
        output_tokens: 900,
        reasoning_output_tokens: 150,
        total_tokens: 2_250,
    };

    let captured_at = chrono::Local
        .with_ymd_and_hms(2024, 1, 2, 3, 4, 5)
        .single()
        .expect("timestamp");

    let model_slug = get_model_offline_for_tests(config.model.as_deref());
    let token_info = token_info_for(&model_slug, &config, &usage);

    let reasoning_effort_override = Some(Some(ReasoningEffort::High));
    let composite = new_status_output(
        &config,
        Some(&token_info),
        &usage,
        &None,
        /*thread_name*/ None,
        /*forked_from*/ None,
        captured_at,
        &model_slug,
        /*collaboration_mode*/ None,
        reasoning_effort_override,
    );
    let mut rendered_lines = render_lines(&composite.display_lines(/*width*/ 80));
    if cfg!(windows) {
        for line in &mut rendered_lines {
            *line = line.replace('\\', "/");
        }
    }
    let sanitized = sanitize_directory(rendered_lines).join("\n");
    assert_snapshot!(sanitized);
}

#[tokio::test]
async fn status_permissions_non_default_workspace_write_uses_workspace_label() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config.model = Some("kimi-for-coding".to_string());
    config.model_provider_id = "kimi".to_string();
    config
        .permissions
        .approval_policy
        .set(AskForApproval::OnRequest.to_core())
        .expect("set approval policy");
    set_workspace_cwd(&mut config, test_path_buf("/workspace/tests").abs());
    config
        .permissions
        .set_permission_profile(app_server_workspace_write_profile(
            /*network_enabled*/ true,
        ))
        .expect("set permission profile");

    assert_eq!(
        permissions_text_for(&config).as_deref(),
        Some("Custom (workspace with network access, Ask for approval)")
    );
}

#[tokio::test]
async fn status_permissions_named_read_only_profile_shows_builtin_label() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config
        .permissions
        .approval_policy
        .set(AskForApproval::OnRequest.to_core())
        .expect("set approval policy");
    config
        .permissions
        .set_permission_profile_from_session_snapshot(PermissionProfileSnapshot::active(
            PermissionProfile::read_only(),
            ActivePermissionProfile::read_only(),
        ))
        .expect("set permission profile");

    assert_eq!(
        permissions_text_for(&config).as_deref(),
        Some("Read Only (Ask for approval)")
    );
}

#[tokio::test]
async fn status_permissions_read_only_profile_shows_additional_writable_roots() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config
        .permissions
        .approval_policy
        .set(AskForApproval::OnRequest.to_core())
        .expect("set approval policy");
    let extra_root = test_path_buf("/workspace/extra").abs();
    let file_system_policy = PermissionProfile::read_only()
        .file_system_sandbox_policy()
        .with_additional_writable_roots(config.cwd.as_path(), std::slice::from_ref(&extra_root));
    config
        .permissions
        .set_permission_profile_from_session_snapshot(PermissionProfileSnapshot::active(
            PermissionProfile::from_runtime_permissions(
                &file_system_policy,
                NetworkSandboxPolicy::Restricted,
            ),
            ActivePermissionProfile::read_only(),
        ))
        .expect("set permission profile");

    assert_eq!(
        permissions_text_for(&config).as_deref(),
        Some("Read Only (Ask for approval)")
    );
}

#[tokio::test]
async fn status_permissions_named_workspace_profile_shows_builtin_label() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config
        .permissions
        .approval_policy
        .set(AskForApproval::OnRequest.to_core())
        .expect("set approval policy");
    config
        .permissions
        .set_permission_profile_from_session_snapshot(PermissionProfileSnapshot::active(
            PermissionProfile::workspace_write(),
            ActivePermissionProfile::new(BUILT_IN_PERMISSION_PROFILE_WORKSPACE),
        ))
        .expect("set permission profile");

    assert_eq!(
        permissions_text_for(&config).as_deref(),
        Some("Workspace (Ask for approval)")
    );
}

#[tokio::test]
async fn status_permissions_workspace_auto_review_shows_reviewer_label() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config.approvals_reviewer = ApprovalsReviewer::AutoReview;
    config
        .permissions
        .approval_policy
        .set(AskForApproval::OnRequest.to_core())
        .expect("set approval policy");
    config
        .permissions
        .set_permission_profile_from_session_snapshot(PermissionProfileSnapshot::active(
            PermissionProfile::workspace_write(),
            ActivePermissionProfile::new(BUILT_IN_PERMISSION_PROFILE_WORKSPACE),
        ))
        .expect("set permission profile");

    assert_eq!(
        permissions_text_for(&config).as_deref(),
        Some("Workspace (Approve for me)")
    );
}

#[tokio::test]
async fn status_permissions_named_profile_shows_additional_writable_roots() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config
        .permissions
        .approval_policy
        .set(AskForApproval::OnRequest.to_core())
        .expect("set approval policy");
    let extra_root = test_path_buf("/workspace/extra").abs();
    config
        .permissions
        .set_permission_profile_from_session_snapshot(PermissionProfileSnapshot::active(
            PermissionProfile::workspace_write_with(
                std::slice::from_ref(&extra_root),
                NetworkSandboxPolicy::Restricted,
                /*exclude_tmpdir_env_var*/ false,
                /*exclude_slash_tmp*/ false,
            ),
            ActivePermissionProfile::new(BUILT_IN_PERMISSION_PROFILE_WORKSPACE),
        ))
        .expect("set permission profile");

    assert_eq!(
        permissions_text_for(&config).as_deref(),
        Some("Workspace (Ask for approval)")
    );
}

#[tokio::test]
async fn status_permissions_workspace_roots_show_additional_directories() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    set_workspace_cwd(&mut config, test_path_buf("/workspace/tests").abs());
    config
        .permissions
        .approval_policy
        .set(AskForApproval::OnRequest.to_core())
        .expect("set approval policy");
    let extra_root = test_path_buf("/workspace/extra").abs();
    config.workspace_roots = vec![config.cwd.clone(), extra_root.clone()];
    config
        .permissions
        .set_workspace_roots(config.workspace_roots.clone());
    config
        .permissions
        .set_permission_profile_from_session_snapshot(PermissionProfileSnapshot::active(
            PermissionProfile::workspace_write(),
            ActivePermissionProfile::new(":workspace"),
        ))
        .expect("set permission profile");

    assert_eq!(
        permissions_text_for(&config),
        Some(format!(
            "Workspace [{}] (Ask for approval)",
            extra_root.display()
        ))
    );
}

#[tokio::test]
async fn status_permissions_workspace_roots_include_profile_defined_directories() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    set_workspace_cwd(&mut config, test_path_buf("/workspace/tests").abs());
    config
        .permissions
        .approval_policy
        .set(AskForApproval::OnRequest.to_core())
        .expect("set approval policy");
    let profile_root = test_path_buf("/workspace/shared").abs();
    config
        .permissions
        .set_permission_profile_from_session_snapshot(
            PermissionProfileSnapshot::active_with_profile_workspace_roots(
                PermissionProfile::workspace_write_with(
                    std::slice::from_ref(&profile_root),
                    NetworkSandboxPolicy::Restricted,
                    /*exclude_tmpdir_env_var*/ false,
                    /*exclude_slash_tmp*/ false,
                ),
                ActivePermissionProfile::new(":workspace"),
                vec![profile_root.clone()],
            ),
        )
        .expect("set permission profile");

    assert_eq!(
        permissions_text_for(&config),
        Some(format!(
            "Workspace [{}] (Ask for approval)",
            profile_root.display()
        ))
    );
}

#[tokio::test]
async fn status_permissions_broadened_workspace_profile_shows_builtin_label() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config
        .permissions
        .approval_policy
        .set(AskForApproval::OnRequest.to_core())
        .expect("set approval policy");
    config
        .permissions
        .set_permission_profile_from_session_snapshot(PermissionProfileSnapshot::active(
            PermissionProfile::workspace_write_with(
                &[],
                NetworkSandboxPolicy::Enabled,
                /*exclude_tmpdir_env_var*/ false,
                /*exclude_slash_tmp*/ false,
            ),
            ActivePermissionProfile::new(BUILT_IN_PERMISSION_PROFILE_WORKSPACE),
        ))
        .expect("set permission profile");

    assert_eq!(
        permissions_text_for(&config).as_deref(),
        Some("Workspace with network access (Ask for approval)")
    );
}

#[tokio::test]
async fn status_permissions_user_defined_profile_shows_name() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config
        .permissions
        .set_permission_profile_from_session_snapshot(PermissionProfileSnapshot::active(
            PermissionProfile::read_only(),
            ActivePermissionProfile::new("locked"),
        ))
        .expect("set permission profile");

    assert_eq!(
        permissions_text_for(&config).as_deref(),
        Some("Profile locked (read-only, Ask for approval)")
    );
}

#[tokio::test]
async fn status_snapshot_shows_active_user_defined_profile() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config.model = Some("kimi-for-coding".to_string());
    set_workspace_cwd(&mut config, test_path_buf("/workspace/tests").abs());
    config
        .permissions
        .set_permission_profile_from_session_snapshot(PermissionProfileSnapshot::active(
            PermissionProfile::read_only(),
            ActivePermissionProfile::new("locked"),
        ))
        .expect("set permission profile");

    let usage = TokenUsage::default();
    let captured_at = chrono::Local
        .with_ymd_and_hms(2024, 1, 2, 3, 4, 5)
        .single()
        .expect("timestamp");
    let model_slug = get_model_offline_for_tests(config.model.as_deref());
    let token_info = token_info_for(&model_slug, &config, &usage);

    let composite = new_status_output(
        &config,
        Some(&token_info),
        &usage,
        &None,
        /*thread_name*/ None,
        /*forked_from*/ None,
        captured_at,
        &model_slug,
        /*collaboration_mode*/ None,
        /*reasoning_effort_override*/ None,
    );
    let mut rendered_lines = render_lines(&composite.display_lines(/*width*/ 80));
    if cfg!(windows) {
        for line in &mut rendered_lines {
            *line = line.replace('\\', "/");
        }
    }
    let sanitized = sanitize_directory(rendered_lines).join("\n");
    assert_snapshot!(sanitized);
}

#[tokio::test]
async fn status_snapshot_shows_auto_review_permissions() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config.model = Some("kimi-for-coding".to_string());
    set_workspace_cwd(&mut config, test_path_buf("/workspace/tests").abs());
    config.approvals_reviewer = ApprovalsReviewer::AutoReview;
    config
        .permissions
        .set_permission_profile_from_session_snapshot(PermissionProfileSnapshot::active(
            PermissionProfile::workspace_write(),
            ActivePermissionProfile::new(BUILT_IN_PERMISSION_PROFILE_WORKSPACE),
        ))
        .expect("set permission profile");

    let usage = TokenUsage::default();
    let captured_at = chrono::Local
        .with_ymd_and_hms(2024, 1, 2, 3, 4, 5)
        .single()
        .expect("timestamp");
    let model_slug = get_model_offline_for_tests(config.model.as_deref());
    let token_info = token_info_for(&model_slug, &config, &usage);

    let composite = new_status_output(
        &config,
        Some(&token_info),
        &usage,
        &None,
        /*thread_name*/ None,
        /*forked_from*/ None,
        captured_at,
        &model_slug,
        /*collaboration_mode*/ None,
        /*reasoning_effort_override*/ None,
    );
    let mut rendered_lines = render_lines(&composite.display_lines(/*width*/ 80));
    if cfg!(windows) {
        for line in &mut rendered_lines {
            *line = line.replace('\\', "/");
        }
    }
    let sanitized = sanitize_directory(rendered_lines).join("\n");
    assert_snapshot!(sanitized);
}

#[tokio::test]
async fn status_permissions_full_disk_managed_with_network_is_danger_full_access() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config
        .permissions
        .approval_policy
        .set(AskForApproval::OnRequest.to_core())
        .expect("set approval policy");
    config
        .permissions
        .set_permission_profile(PermissionProfile::Managed {
            network: NetworkSandboxPolicy::Enabled,
            file_system: ManagedFileSystemPermissions::Unrestricted,
        })
        .expect("set permission profile");

    assert_eq!(
        permissions_text_for(&config).as_deref(),
        Some("Custom (danger-full-access, Ask for approval)")
    );
}

#[tokio::test]
async fn status_permissions_full_disk_managed_without_network_is_external_sandbox() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config
        .permissions
        .approval_policy
        .set(AskForApproval::OnRequest.to_core())
        .expect("set approval policy");
    config
        .permissions
        .set_permission_profile(PermissionProfile::Managed {
            network: NetworkSandboxPolicy::Restricted,
            file_system: ManagedFileSystemPermissions::Unrestricted,
        })
        .expect("set permission profile");

    assert_eq!(
        permissions_text_for(&config).as_deref(),
        Some("Custom (external-sandbox, Ask for approval)")
    );
}

#[tokio::test]
async fn status_snapshot_includes_forked_from() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config.model = Some("kimi-for-coding".to_string());
    config.model_provider_id = "kimi".to_string();
    set_workspace_cwd(&mut config, test_path_buf("/workspace/tests").abs());

    let usage = TokenUsage {
        input_tokens: 800,
        cached_input_tokens: 0,
        output_tokens: 400,
        reasoning_output_tokens: 0,
        total_tokens: 1_200,
    };

    let captured_at = chrono::Local
        .with_ymd_and_hms(2024, 8, 9, 10, 11, 12)
        .single()
        .expect("valid time");

    let model_slug = get_model_offline_for_tests(config.model.as_deref());
    let token_info = token_info_for(&model_slug, &config, &usage);
    let session_id =
        ThreadId::from_string("0f0f3c13-6cf9-4aa4-8b80-7d49c2f1be2e").expect("session id");
    let forked_from =
        ThreadId::from_string("e9f18a88-8081-4e51-9d4e-8af5cde2d8dd").expect("forked id");

    let composite = new_status_output(
        &config,
        Some(&token_info),
        &usage,
        &Some(session_id),
        /*thread_name*/ None,
        Some(forked_from),
        captured_at,
        &model_slug,
        /*collaboration_mode*/ None,
        /*reasoning_effort_override*/ None,
    );
    let mut rendered_lines = render_lines(&composite.display_lines(/*width*/ 80));
    if cfg!(windows) {
        for line in &mut rendered_lines {
            *line = line.replace('\\', "/");
        }
    }
    let sanitized = sanitize_directory(rendered_lines).join("\n");
    assert_snapshot!(sanitized);
}

#[tokio::test]
async fn status_card_token_usage_excludes_cached_tokens() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config.model = Some("kimi-for-coding".to_string());
    set_workspace_cwd(&mut config, test_path_buf("/workspace/tests").abs());

    let usage = TokenUsage {
        input_tokens: 1_200,
        cached_input_tokens: 200,
        output_tokens: 900,
        reasoning_output_tokens: 0,
        total_tokens: 2_100,
    };

    let now = chrono::Local
        .with_ymd_and_hms(2024, 1, 1, 0, 0, 0)
        .single()
        .expect("timestamp");

    let model_slug = get_model_offline_for_tests(config.model.as_deref());
    let token_info = token_info_for(&model_slug, &config, &usage);
    let composite = new_status_output(
        &config,
        Some(&token_info),
        &usage,
        &None,
        /*thread_name*/ None,
        /*forked_from*/ None,
        now,
        &model_slug,
        /*collaboration_mode*/ None,
        /*reasoning_effort_override*/ None,
    );
    let rendered = render_lines(&composite.display_lines(/*width*/ 120));

    assert!(
        rendered.iter().all(|line| !line.contains("cached")),
        "cached tokens should not be displayed, got: {rendered:?}"
    );
}

#[tokio::test]
async fn status_snapshot_shows_missing_limits_message() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config.model = Some("kimi-for-coding".to_string());
    set_workspace_cwd(&mut config, test_path_buf("/workspace/tests").abs());

    let usage = TokenUsage {
        input_tokens: 500,
        cached_input_tokens: 0,
        output_tokens: 250,
        reasoning_output_tokens: 0,
        total_tokens: 750,
    };

    let now = chrono::Local
        .with_ymd_and_hms(2024, 2, 3, 4, 5, 6)
        .single()
        .expect("timestamp");

    let model_slug = get_model_offline_for_tests(config.model.as_deref());
    let token_info = token_info_for(&model_slug, &config, &usage);
    let composite = new_status_output(
        &config,
        Some(&token_info),
        &usage,
        &None,
        /*thread_name*/ None,
        /*forked_from*/ None,
        now,
        &model_slug,
        /*collaboration_mode*/ None,
        /*reasoning_effort_override*/ None,
    );
    let mut rendered_lines = render_lines(&composite.display_lines(/*width*/ 80));
    if cfg!(windows) {
        for line in &mut rendered_lines {
            *line = line.replace('\\', "/");
        }
    }
    let sanitized = sanitize_directory(rendered_lines).join("\n");
    assert_snapshot!(sanitized);
}

#[tokio::test]
async fn status_snapshot_uses_default_reasoning_when_config_empty() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config.model = Some("kimi-for-coding".to_string());
    set_workspace_cwd(&mut config, test_path_buf("/workspace/tests").abs());

    let usage = TokenUsage {
        input_tokens: 500,
        cached_input_tokens: 0,
        output_tokens: 250,
        reasoning_output_tokens: 0,
        total_tokens: 750,
    };

    let now = chrono::Local
        .with_ymd_and_hms(2024, 2, 3, 4, 5, 6)
        .single()
        .expect("timestamp");
    let remote_connection = RemoteConnectionStatus {
        address: "unix:///tmp/ody-home/app-server-control/app-server-control.sock".to_string(),
        version: "v0.133.0".to_string(),
    };

    let model_slug = get_model_offline_for_tests(config.model.as_deref());
    let token_info = token_info_for(&model_slug, &config, &usage);
    let (composite, _) = new_status_output_with_handle(
        &config,
        /*runtime_model_provider_base_url*/ None,
        Some(&remote_connection),
        Some(&token_info),
        &usage,
        &None,
        /*thread_name*/ None,
        /*forked_from*/ None,
        now,
        &model_slug,
        /*collaboration_mode*/ None,
        /*reasoning_effort_override*/ Some(Some(ReasoningEffort::Medium)),
        "<none>".to_string(),
    );
    let mut rendered_lines = render_lines(&composite.display_lines(/*width*/ 80));
    if cfg!(windows) {
        for line in &mut rendered_lines {
            *line = line.replace('\\', "/");
        }
    }
    let sanitized = sanitize_directory(rendered_lines).join("\n");
    assert_snapshot!(sanitized);
}

#[tokio::test]
async fn status_context_window_uses_last_usage() {
    let temp_home = TempDir::new().expect("temp home");
    let mut config = test_config(&temp_home).await;
    config.model_context_window = Some(272_000);

    let total_usage = TokenUsage {
        input_tokens: 12_800,
        cached_input_tokens: 0,
        output_tokens: 879,
        reasoning_output_tokens: 0,
        total_tokens: 102_000,
    };
    let last_usage = TokenUsage {
        input_tokens: 12_800,
        cached_input_tokens: 0,
        output_tokens: 879,
        reasoning_output_tokens: 0,
        total_tokens: 13_679,
    };

    let now = chrono::Local
        .with_ymd_and_hms(2024, 6, 1, 12, 0, 0)
        .single()
        .expect("timestamp");

    let model_slug = get_model_offline_for_tests(config.model.as_deref());
    let token_info = TokenUsageInfo {
        total_token_usage: total_usage.clone(),
        last_token_usage: last_usage,
        model_context_window: config.model_context_window,
        auto_compact_token_limit: None,
    };
    let composite = new_status_output(
        &config,
        Some(&token_info),
        &total_usage,
        &None,
        /*thread_name*/ None,
        /*forked_from*/ None,
        now,
        &model_slug,
        /*collaboration_mode*/ None,
        /*reasoning_effort_override*/ None,
    );
    let rendered_lines = render_lines(&composite.display_lines(/*width*/ 80));
    let context_line = rendered_lines
        .into_iter()
        .find(|line| line.contains("Context window"))
        .expect("context line");

    assert!(
        context_line.contains("13.7K used / 272K"),
        "expected context line to reflect last usage tokens, got: {context_line}"
    );
    assert!(
        !context_line.contains("102K"),
        "context line should not use total aggregated tokens, got: {context_line}"
    );
}
