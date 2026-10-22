//! Shared command-line flags used by both interactive and non-interactive Ody entry points.

use crate::SandboxModeCliArg;
use clap::Args;
use ody_protocol::config_types::ProfileV2Name;
use std::path::PathBuf;

#[derive(Args, Clone, Debug, Default)]
pub struct SharedCliOptions {
    /// Optional image(s) to attach to the initial prompt.
    #[arg(
        long = "image",
        short = 'i',
        value_name = "FILE",
        value_delimiter = ',',
        num_args = 1..
    )]
    pub images: Vec<PathBuf>,

    /// Model the agent should use.
    #[arg(long, short = 'm')]
    pub model: Option<String>,

    /// Layer $ODY_HOME/<name>.config.toml on top of the base user config.
    #[arg(long = "profile", short = 'p')]
    pub config_profile_v2: Option<ProfileV2Name>,

    /// Select the sandbox policy to use when executing model-generated shell
    /// commands.
    #[arg(long = "sandbox", short = 's')]
    pub sandbox_mode: Option<SandboxModeCliArg>,

    /// Skip all confirmation prompts and execute commands without sandboxing.
    /// EXTREMELY DANGEROUS. Intended solely for running in environments that are externally sandboxed.
    #[arg(
        long = "dangerously-bypass-approvals-and-sandbox",
        alias = "yolo",
        default_value_t = false
    )]
    pub dangerously_bypass_approvals_and_sandbox: bool,

    /// Run enabled hooks without requiring persisted hook trust for this invocation.
    /// DANGEROUS. Intended only for automation that already vets hook sources.
    #[arg(long = "dangerously-bypass-hook-trust", default_value_t = false)]
    pub bypass_hook_trust: bool,

    /// Tell the agent to use the specified directory as its working root.
    #[clap(long = "cd", short = 'C', value_name = "DIR")]
    pub cwd: Option<PathBuf>,

    /// Additional directories that should be writable alongside the primary workspace.
    #[arg(long = "add-dir", value_name = "DIR", value_hint = clap::ValueHint::DirPath)]
    pub add_dir: Vec<PathBuf>,

    /// Enable the bundled game-development skills (the `/game` command and the
    /// vendored gamedev skill set) for this invocation. Off by default to keep
    /// the skills context budget small.
    #[arg(long = "game", default_value_t = false)]
    pub game: bool,

    /// Restrict this invocation to the bundled frontend-design craft skills
    /// (`make-interfaces-feel-better` and `motion-design`). Exclusive: every
    /// other skill, from any scope, is hidden while the flag is set.
    #[arg(long = "frontend", default_value_t = false)]
    pub frontend: bool,
}

impl SharedCliOptions {
    /// Extra `-c`-style config overrides implied by dedicated flags, in
    /// addition to the user's own `-c key=value` overrides. The [`--game`]
    /// flag layers `skills.bundled.game = true` onto the config stack, and
    /// [`--frontend`] layers `skills.bundled.frontend = true`.
    ///
    /// [`--game`]: Self::game
    /// [`--frontend`]: Self::frontend
    pub fn implied_config_overrides(&self) -> Vec<(String, toml::Value)> {
        let mut overrides = Vec::new();
        if self.game {
            overrides.push((
                "skills.bundled.game".to_string(),
                toml::Value::Boolean(true),
            ));
        }
        if self.frontend {
            overrides.push((
                "skills.bundled.frontend".to_string(),
                toml::Value::Boolean(true),
            ));
        }
        overrides
    }

    pub fn inherit_exec_root_options(&mut self, root: &Self) {
        let self_selected_sandbox_mode =
            self.sandbox_mode.is_some() || self.dangerously_bypass_approvals_and_sandbox;
        let Self {
            images,
            model,
            config_profile_v2,
            sandbox_mode,
            dangerously_bypass_approvals_and_sandbox,
            bypass_hook_trust,
            cwd,
            add_dir,
            game,
            frontend,
        } = self;
        let Self {
            images: root_images,
            model: root_model,
            config_profile_v2: root_config_profile_v2,
            sandbox_mode: root_sandbox_mode,
            dangerously_bypass_approvals_and_sandbox: root_dangerously_bypass_approvals_and_sandbox,
            bypass_hook_trust: root_bypass_hook_trust,
            cwd: root_cwd,
            add_dir: root_add_dir,
            game: root_game,
            frontend: root_frontend,
        } = root;

        if model.is_none() {
            model.clone_from(root_model);
        }
        if config_profile_v2.is_none() {
            config_profile_v2.clone_from(root_config_profile_v2);
        }
        if sandbox_mode.is_none() {
            *sandbox_mode = *root_sandbox_mode;
        }
        if !self_selected_sandbox_mode {
            *dangerously_bypass_approvals_and_sandbox =
                *root_dangerously_bypass_approvals_and_sandbox;
        }
        if !*bypass_hook_trust {
            *bypass_hook_trust = *root_bypass_hook_trust;
        }
        if cwd.is_none() {
            cwd.clone_from(root_cwd);
        }
        if !root_images.is_empty() {
            let mut merged_images = root_images.clone();
            merged_images.append(images);
            *images = merged_images;
        }
        if !root_add_dir.is_empty() {
            let mut merged_add_dir = root_add_dir.clone();
            merged_add_dir.append(add_dir);
            *add_dir = merged_add_dir;
        }
        if !*game {
            *game = *root_game;
        }
        if !*frontend {
            *frontend = *root_frontend;
        }
    }

    pub fn apply_subcommand_overrides(&mut self, subcommand: Self) {
        let subcommand_selected_sandbox_mode = subcommand.sandbox_mode.is_some()
            || subcommand.dangerously_bypass_approvals_and_sandbox;
        let Self {
            images,
            model,
            config_profile_v2,
            sandbox_mode,
            dangerously_bypass_approvals_and_sandbox,
            bypass_hook_trust,
            cwd,
            add_dir,
            game,
            frontend,
        } = subcommand;

        if let Some(model) = model {
            self.model = Some(model);
        }
        if let Some(config_profile_v2) = config_profile_v2 {
            self.config_profile_v2 = Some(config_profile_v2);
        }
        if subcommand_selected_sandbox_mode {
            self.sandbox_mode = sandbox_mode;
            self.dangerously_bypass_approvals_and_sandbox =
                dangerously_bypass_approvals_and_sandbox;
        }
        if bypass_hook_trust {
            self.bypass_hook_trust = true;
        }
        if let Some(cwd) = cwd {
            self.cwd = Some(cwd);
        }
        if !images.is_empty() {
            self.images = images;
        }
        if !add_dir.is_empty() {
            self.add_dir.extend(add_dir);
        }
        if game {
            self.game = true;
        }
        if frontend {
            self.frontend = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct TestCli {
        #[clap(flatten)]
        shared: SharedCliOptions,
    }

    #[test]
    fn game_flag_defaults_to_off() {
        let cli = TestCli::try_parse_from(["test"]).expect("parse");
        assert!(!cli.shared.game);
        assert!(!cli.shared.frontend);
        assert!(cli.shared.implied_config_overrides().is_empty());
    }

    #[test]
    fn game_flag_generates_skills_config_override() {
        let cli = TestCli::try_parse_from(["test", "--game"]).expect("parse");
        assert!(cli.shared.game);
        assert_eq!(
            cli.shared.implied_config_overrides(),
            vec![(
                "skills.bundled.game".to_string(),
                toml::Value::Boolean(true),
            )]
        );
    }

    #[test]
    fn inherit_exec_root_options_merges_game_flag() {
        let mut child = SharedCliOptions::default();
        let root = SharedCliOptions {
            game: true,
            ..Default::default()
        };
        child.inherit_exec_root_options(&root);
        assert!(child.game);

        // An explicit child selection is never clobbered by the root.
        let mut child = SharedCliOptions {
            game: true,
            ..Default::default()
        };
        child.inherit_exec_root_options(&SharedCliOptions::default());
        assert!(child.game);
    }

    #[test]
    fn apply_subcommand_overrides_merges_game_flag() {
        let mut root = SharedCliOptions::default();
        root.apply_subcommand_overrides(SharedCliOptions {
            game: true,
            ..Default::default()
        });
        assert!(root.game);
    }

    #[test]
    fn frontend_flag_generates_skills_config_override() {
        let cli = TestCli::try_parse_from(["test", "--frontend"]).expect("parse");
        assert!(cli.shared.frontend);
        assert!(!cli.shared.game, "--frontend must not imply --game");
        assert_eq!(
            cli.shared.implied_config_overrides(),
            vec![(
                "skills.bundled.frontend".to_string(),
                toml::Value::Boolean(true),
            )]
        );
    }

    #[test]
    fn frontend_and_game_flags_compose() {
        let cli = TestCli::try_parse_from(["test", "--game", "--frontend"]).expect("parse");
        assert_eq!(
            cli.shared.implied_config_overrides(),
            vec![
                (
                    "skills.bundled.game".to_string(),
                    toml::Value::Boolean(true),
                ),
                (
                    "skills.bundled.frontend".to_string(),
                    toml::Value::Boolean(true),
                ),
            ]
        );
    }

    #[test]
    fn inherit_exec_root_options_merges_frontend_flag() {
        let mut child = SharedCliOptions::default();
        let root = SharedCliOptions {
            frontend: true,
            ..Default::default()
        };
        child.inherit_exec_root_options(&root);
        assert!(child.frontend);

        // An explicit child selection is never clobbered by the root.
        let mut child = SharedCliOptions {
            frontend: true,
            ..Default::default()
        };
        child.inherit_exec_root_options(&SharedCliOptions::default());
        assert!(child.frontend);
    }

    #[test]
    fn apply_subcommand_overrides_merges_frontend_flag() {
        let mut root = SharedCliOptions::default();
        root.apply_subcommand_overrides(SharedCliOptions {
            frontend: true,
            ..Default::default()
        });
        assert!(root.frontend);
    }
}
