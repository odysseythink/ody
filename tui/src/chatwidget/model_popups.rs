//! Model, collaboration, and reasoning popups for `ChatWidget`.
//!
//! These surfaces are tightly related because changing one often redirects
//! into another, especially while Plan mode is active.

use super::*;
use ratatui::buffer::Buffer;
use ratatui::text::{Line, Span};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Availability of the thinking toggle for a given model preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThinkingAvailability {
    AlwaysOn,
    Toggle,
    Unsupported,
}

/// Shared mutable state for the model picker: the current thinking draft.
struct ModelPickerState {
    thinking: AtomicBool,
    availability: AtomicBool,
}

impl ModelPickerState {
    fn new(thinking: bool, availability: ThinkingAvailability) -> Self {
        let (thinking_val, availability_val) = match availability {
            ThinkingAvailability::AlwaysOn => (true, 0),
            ThinkingAvailability::Toggle => (thinking, 1),
            ThinkingAvailability::Unsupported => (false, 2),
        };
        Self {
            thinking: AtomicBool::new(thinking_val),
            availability: AtomicBool::new(availability_val == 1),
        }
    }

    fn toggle_thinking(&self) {
        if self.is_toggle() {
            self.thinking.fetch_not(Ordering::SeqCst);
        }
    }

    fn thinking(&self) -> bool {
        self.thinking.load(Ordering::SeqCst)
    }

    fn availability(&self) -> ThinkingAvailability {
        if self.availability.load(Ordering::SeqCst) {
            ThinkingAvailability::Toggle
        } else if self.thinking.load(Ordering::SeqCst) {
            ThinkingAvailability::AlwaysOn
        } else {
            ThinkingAvailability::Unsupported
        }
    }

    fn is_toggle(&self) -> bool {
        self.availability.load(Ordering::SeqCst)
    }
}

/// Header that renders the model picker title, subtitle, hint, and optional
/// base-URL override warning.
struct ModelPickerHeader {
    title: String,
    subtitle: String,
    warning: Option<Line<'static>>,
    hint: Line<'static>,
}

impl Renderable for ModelPickerHeader {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(self.title.clone().bold()));
        lines.push(Line::from(self.subtitle.clone().dim()));
        if let Some(warning) = &self.warning {
            lines.push(warning.clone());
        }
        lines.push(self.hint.clone().dim());
        Paragraph::new(lines).render(area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        let mut height = 3u16;
        if self.warning.is_some() {
            height += 1;
        }
        height
    }
}

/// Footer-style control that renders the thinking toggle state at the bottom of
/// the model picker so it updates live when `/` is pressed.
struct ModelPickerThinkingControl {
    state: Arc<ModelPickerState>,
}

impl Renderable for ModelPickerThinkingControl {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let label = match self.state.availability() {
            ThinkingAvailability::AlwaysOn => "Thinking  (/ to toggle)   [ Always on ]".to_string(),
            ThinkingAvailability::Unsupported => {
                "Thinking  (/ to toggle)   [ Off ] unsupported".to_string()
            }
            ThinkingAvailability::Toggle => {
                if self.state.thinking() {
                    "Thinking  (/ to toggle)   [ On ]  Off".to_string()
                } else {
                    "Thinking  (/ to toggle)   On   [ Off ]".to_string()
                }
            }
        };
        Line::from(label).render(area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

impl ChatWidget {
    /// Open the tabbed model picker aligned with the upstream `/model` UX.
    pub(crate) fn open_model_popup(&mut self) {
        if !self.is_session_configured() {
            self.add_info_message(
                "Model selection is disabled until startup completes.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        let presets: Vec<ModelPreset> = match self.model_catalog.try_list_models() {
            Ok(models) => models,
            Err(_) => {
                self.add_info_message(
                    "Models are being updated; please try /model again in a moment.".to_string(),
                    /*hint*/ None,
                );
                return;
            }
        };
        self.open_model_picker(presets, None);
    }

    /// Open the model picker with a specific model pre-selected.
    /// If the model is not available, shows an error and falls back to the default picker.
    pub(crate) fn open_model_popup_with_selected(&mut self, selected_model: &str) {
        if !self.is_session_configured() {
            self.add_info_message(
                "Model selection is disabled until startup completes.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        let presets: Vec<ModelPreset> = match self.model_catalog.try_list_models() {
            Ok(models) => models.into_iter().filter(|p| p.show_in_picker).collect(),
            Err(_) => {
                self.add_info_message(
                    "Models are being updated; please try /model again in a moment.".to_string(),
                    /*hint*/ None,
                );
                return;
            }
        };

        let selected_exists = presets.iter().any(|p| p.model.as_str() == selected_model);
        if !selected_exists {
            self.add_error_message(format!("Unknown model alias: {selected_model}"));
            return;
        }

        self.open_model_picker(presets, Some(selected_model));
    }

    /// Open a tabbed model picker with a specific list of models.
    pub(crate) fn open_all_models_popup(
        &mut self,
        models: Vec<ModelPreset>,
        selected_model: Option<&str>,
    ) {
        self.open_model_picker(models, selected_model);
    }

    /// Test helper to open the model picker with a specific set of presets.
    #[cfg(test)]
    pub(crate) fn open_model_popup_with_presets(&mut self, presets: Vec<ModelPreset>) {
        self.open_model_picker(presets, None);
    }

    fn provider_display_name(provider: &str) -> String {
        match provider {
            "kimi" => "Kimi".to_string(),
            "deepseek" => "DeepSeek".to_string(),
            "glm" => "GLM".to_string(),
            "openai" => "OpenAI".to_string(),
            "anthropic" => "Anthropic".to_string(),
            "google" => "Google".to_string(),
            _ => provider.to_string(),
        }
    }

    fn thinking_availability(preset: &ModelPreset) -> ThinkingAvailability {
        let has_none = preset
            .supported_reasoning_efforts
            .iter()
            .any(|o| o.effort == ReasoningEffortConfig::None);
        let has_thinking = preset
            .supported_reasoning_efforts
            .iter()
            .any(|o| o.effort != ReasoningEffortConfig::None);

        if !has_thinking {
            ThinkingAvailability::Unsupported
        } else if !has_none {
            ThinkingAvailability::AlwaysOn
        } else {
            ThinkingAvailability::Toggle
        }
    }

    fn reasoning_effort_for_thinking(
        preset: &ModelPreset,
        thinking: bool,
    ) -> Option<ReasoningEffortConfig> {
        let thinking_efforts: Vec<_> = preset
            .supported_reasoning_efforts
            .iter()
            .filter(|o| o.effort != ReasoningEffortConfig::None)
            .collect();

        match Self::thinking_availability(preset) {
            ThinkingAvailability::Unsupported => Some(ReasoningEffortConfig::None),
            ThinkingAvailability::AlwaysOn => {
                if thinking_efforts.len() == 1 {
                    Some(thinking_efforts[0].effort.clone())
                } else if preset.default_reasoning_effort != ReasoningEffortConfig::None {
                    Some(preset.default_reasoning_effort.clone())
                } else {
                    thinking_efforts.first().map(|o| o.effort.clone())
                }
            }
            ThinkingAvailability::Toggle => {
                if !thinking {
                    Some(ReasoningEffortConfig::None)
                } else if thinking_efforts.len() == 1 {
                    Some(thinking_efforts[0].effort.clone())
                } else if preset.default_reasoning_effort != ReasoningEffortConfig::None {
                    Some(preset.default_reasoning_effort.clone())
                } else {
                    thinking_efforts.first().map(|o| o.effort.clone())
                }
            }
        }
    }

    /// Build a model picker item for the tabbed view.
    fn build_model_picker_item(
        &self,
        preset: &ModelPreset,
        current_model: &str,
        state: Arc<ModelPickerState>,
        should_prompt_plan_mode_scope: bool,
    ) -> SelectionItem {
        let description =
            (!preset.description.is_empty()).then_some(preset.description.to_string());
        let model = preset.model.clone();
        let is_current = preset.model.as_str() == current_model;
        let model_for_action = model.clone();
        let preset_for_action = preset.clone();
        let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            let effort = Self::reasoning_effort_for_thinking(&preset_for_action, state.thinking());
            if should_prompt_plan_mode_scope {
                tx.send(AppEvent::OpenPlanReasoningScopePrompt {
                    model: model_for_action.clone(),
                    effort,
                });
            } else {
                tx.send(AppEvent::UpdateModel(model_for_action.clone()));
                tx.send(AppEvent::UpdateReasoningEffort(effort.clone()));
                tx.send(AppEvent::PersistModelSelection {
                    model: model_for_action.clone(),
                    effort,
                });
            }
        })];
        SelectionItem {
            name: model,
            description,
            is_current,
            is_default: preset.is_default,
            search_value: Some(format!(
                "{} {} {}",
                preset.model, preset.display_name, preset.provider
            )),
            actions,
            dismiss_on_select: true,
            ..Default::default()
        }
    }

    /// Build the tabbed model picker view.
    fn open_model_picker(&mut self, presets: Vec<ModelPreset>, selected_model: Option<&str>) {
        let presets: Vec<ModelPreset> = presets.into_iter().filter(|p| p.show_in_picker).collect();
        if presets.is_empty() {
            self.add_info_message(
                "No models configured. Run /login to sign in, or /provider to add a provider from a model catalog.".to_string(),
                /*hint*/ None,
            );
            return;
        }

        let current_model = self.current_model().to_string();
        let selected_model = selected_model
            .map(|s| s.to_string())
            .unwrap_or_else(|| current_model.clone());

        // Determine the initial thinking state from the currently selected model.
        let current_preset = presets.iter().find(|p| p.model == current_model);
        let initial_thinking = current_preset
            .map(|p| Self::reasoning_effort_for_thinking(p, true))
            .flatten()
            .is_some();
        let initial_availability = current_preset
            .map(Self::thinking_availability)
            .unwrap_or(ThinkingAvailability::Unsupported);
        let state = Arc::new(ModelPickerState::new(
            initial_thinking,
            initial_availability,
        ));

        // Precompute plan-mode reasoning scope prompts for each preset.
        let mut scope_prompts: HashMap<String, bool> = HashMap::new();
        for preset in &presets {
            let effort = Self::reasoning_effort_for_thinking(preset, state.thinking());
            let should_prompt = self.should_prompt_plan_mode_reasoning_scope(&preset.model, effort);
            scope_prompts.insert(preset.model.clone(), should_prompt);
        }

        // Group presets by provider. Preserve insertion order of first appearance.
        let mut provider_groups: Vec<(String, Vec<&ModelPreset>)> = Vec::new();
        for preset in &presets {
            let provider = if preset.provider.is_empty() {
                "all".to_string()
            } else {
                preset.provider.clone()
            };
            if let Some(pos) = provider_groups.iter().position(|(p, _)| p == &provider) {
                provider_groups[pos].1.push(preset);
            } else {
                provider_groups.push((provider, vec![preset]));
            }
        }

        // The selected model determines the initial active tab.
        let selected_preset = presets.iter().find(|p| p.model == selected_model);
        let initial_provider = selected_preset
            .map(|p| {
                if p.provider.is_empty() {
                    "all".to_string()
                } else {
                    p.provider.clone()
                }
            })
            .unwrap_or_else(|| {
                provider_groups
                    .first()
                    .map(|(id, _)| id.clone())
                    .unwrap_or_else(|| "all".to_string())
            });

        // Build the shared header that is placed on every tab so it remains visible
        // regardless of which provider tab is active.
        let hint = Line::from(
            "↑↓ model · ←→ page · / thinking · Enter apply · Esc cancel · Tab/Shift+Tab provider",
        );
        let header = Box::new(ModelPickerHeader {
            title: "Select a model".to_string(),
            subtitle: "type to search".to_string(),
            warning: self.model_menu_warning_line(),
            hint: hint.clone(),
        });
        let thinking_control = Box::new(ModelPickerThinkingControl {
            state: state.clone(),
        });

        // Build the "All" tab first, then per-provider tabs.
        let mut tabs: Vec<SelectionTab> = Vec::new();
        let mut all_items: Vec<SelectionItem> = Vec::new();
        for (_, group_presets) in &provider_groups {
            for preset in group_presets {
                let should_prompt = *scope_prompts.get(&preset.model).unwrap_or(&false);
                all_items.push(self.build_model_picker_item(
                    preset,
                    &current_model,
                    state.clone(),
                    should_prompt,
                ));
            }
        }
        tabs.push(SelectionTab {
            id: "all".to_string(),
            label: "All".to_string(),
            header: Box::new(ModelPickerHeader {
                title: "Select a model".to_string(),
                subtitle: "type to search".to_string(),
                warning: self.model_menu_warning_line(),
                hint: hint.clone(),
            }),
            items: all_items,
        });
        for (provider_id, group_presets) in &provider_groups {
            // The "All" tab already covers the "all" provider group; avoid a duplicate tab.
            if provider_id == "all" {
                continue;
            }
            let items = group_presets
                .iter()
                .map(|preset| {
                    let should_prompt = *scope_prompts.get(&preset.model).unwrap_or(&false);
                    self.build_model_picker_item(
                        preset,
                        &current_model,
                        state.clone(),
                        should_prompt,
                    )
                })
                .collect();
            tabs.push(SelectionTab {
                id: provider_id.clone(),
                label: Self::provider_display_name(provider_id),
                header: Box::new(ModelPickerHeader {
                    title: "Select a model".to_string(),
                    subtitle: "type to search".to_string(),
                    warning: self.model_menu_warning_line(),
                    hint: hint.clone(),
                }),
                items,
            });
        }

        let initial_tab_id = if tabs.iter().any(|t| t.id == initial_provider) {
            initial_provider
        } else {
            "all".to_string()
        };

        let selected_state = state.clone();
        let custom_handler: CustomKeyHandlerCallback = Some(Box::new(move |key_event, _tx| {
            if key_event.code == KeyCode::Char('/') && key_event.modifiers == KeyModifiers::NONE {
                selected_state.toggle_thinking();
                return true;
            }
            false
        }));

        let initial_selected_idx = presets.iter().position(|p| p.model == selected_model);

        self.bottom_pane.show_selection_view(SelectionViewParams {
            header,
            tabs,
            initial_tab_id: Some(initial_tab_id),
            initial_selected_idx,
            is_searchable: true,
            custom_key_handler: custom_handler,
            stacked_side_content: Some(thinking_control),
            footer_hint: Some(Line::from("")),
            ..Default::default()
        });
    }

    fn model_menu_warning_line(&self) -> Option<Line<'static>> {
        let base_url = self.custom_base_url()?;
        let warning = format!(
            "Warning: OpenAI base URL is overridden to {base_url}. Selecting models may not be supported or work properly."
        );
        Some(Line::from(warning.red()))
    }

    fn custom_base_url(&self) -> Option<String> {
        if false {
            return None;
        }

        let base_url = self.config.model_provider.base_url.as_ref()?;
        let trimmed = base_url.trim();
        if trimmed.is_empty() {
            return None;
        }

        let normalized = trimmed.trim_end_matches('/');
        if normalized == DEFAULT_OPENAI_BASE_URL {
            return None;
        }

        Some(trimmed.to_string())
    }

    fn model_selection_actions(
        model_for_action: String,
        effort_for_action: Option<ReasoningEffortConfig>,
        should_prompt_plan_mode_scope: bool,
    ) -> Vec<SelectionAction> {
        vec![Box::new(move |tx| {
            if should_prompt_plan_mode_scope {
                tx.send(AppEvent::OpenPlanReasoningScopePrompt {
                    model: model_for_action.clone(),
                    effort: effort_for_action.clone(),
                });
                return;
            }

            tx.send(AppEvent::UpdateModel(model_for_action.clone()));
            tx.send(AppEvent::UpdateReasoningEffort(effort_for_action.clone()));
            tx.send(AppEvent::PersistModelSelection {
                model: model_for_action.clone(),
                effort: effort_for_action.clone(),
            });
        })]
    }

    fn should_prompt_plan_mode_reasoning_scope(
        &self,
        selected_model: &str,
        selected_effort: Option<ReasoningEffortConfig>,
    ) -> bool {
        if !self.collaboration_modes_enabled()
            || self.active_mode_kind() != ModeKind::Plan
            || selected_model != self.current_model()
        {
            return false;
        }

        // Prompt whenever the selection is not a true no-op for both:
        // 1) the active Plan-mode effective reasoning, and
        // 2) the stored global defaults that would be updated by the fallback path.
        selected_effort != self.effective_reasoning_effort()
            || selected_model != self.current_collaboration_mode.model()
            || selected_effort != self.current_collaboration_mode.reasoning_effort()
    }

    pub(crate) fn open_plan_reasoning_scope_prompt(
        &mut self,
        model: String,
        effort: Option<ReasoningEffortConfig>,
    ) {
        let reasoning_phrase = match effort.as_ref() {
            Some(ReasoningEffortConfig::None) => "no reasoning".to_string(),
            Some(selected_effort) => {
                format!(
                    "{} reasoning",
                    Self::reasoning_effort_sentence_label(selected_effort)
                )
            }
            None => "the selected reasoning".to_string(),
        };
        let plan_only_description = format!("Always use {reasoning_phrase} in Plan mode.");
        let plan_reasoning_source = if let Some(plan_override) =
            self.config.plan_mode_reasoning_effort.as_ref()
        {
            format!(
                "user-chosen Plan override ({})",
                Self::reasoning_effort_sentence_label(plan_override)
            )
        } else if let Some(plan_mask) = collaboration_modes::plan_mask(self.model_catalog.as_ref())
        {
            match plan_mask
                .reasoning_effort
                .as_ref()
                .and_then(|effort| effort.as_ref())
            {
                Some(plan_effort) => format!(
                    "built-in Plan default ({})",
                    Self::reasoning_effort_sentence_label(plan_effort)
                ),
                None => "built-in Plan default (no reasoning)".to_string(),
            }
        } else {
            "built-in Plan default".to_string()
        };
        let all_modes_description = format!(
            "Set the global default reasoning level and the Plan mode override. This replaces the current {plan_reasoning_source}."
        );
        let subtitle = format!("Choose where to apply {reasoning_phrase}.");

        let plan_only_actions: Vec<SelectionAction> = vec![Box::new({
            let model = model.clone();
            let effort = effort.clone();
            move |tx| {
                tx.send(AppEvent::UpdateModel(model.clone()));
                tx.send(AppEvent::UpdatePlanModeReasoningEffort(effort.clone()));
                tx.send(AppEvent::PersistPlanModeReasoningEffort(effort.clone()));
            }
        })];
        let all_modes_actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
            tx.send(AppEvent::UpdateModel(model.clone()));
            tx.send(AppEvent::UpdateReasoningEffort(effort.clone()));
            tx.send(AppEvent::UpdatePlanModeReasoningEffort(effort.clone()));
            tx.send(AppEvent::PersistPlanModeReasoningEffort(effort.clone()));
            tx.send(AppEvent::PersistModelSelection {
                model: model.clone(),
                effort: effort.clone(),
            });
        })];

        self.bottom_pane.show_selection_view(SelectionViewParams {
            title: Some(PLAN_MODE_REASONING_SCOPE_TITLE.to_string()),
            subtitle: Some(subtitle),
            footer_hint: Some(standard_popup_hint_line()),
            items: vec![
                SelectionItem {
                    name: PLAN_MODE_REASONING_SCOPE_PLAN_ONLY.to_string(),
                    description: Some(plan_only_description),
                    actions: plan_only_actions,
                    dismiss_on_select: true,
                    ..Default::default()
                },
                SelectionItem {
                    name: PLAN_MODE_REASONING_SCOPE_ALL_MODES.to_string(),
                    description: Some(all_modes_description),
                    actions: all_modes_actions,
                    dismiss_on_select: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        self.notify(Notification::PlanModePrompt {
            title: PLAN_MODE_REASONING_SCOPE_TITLE.to_string(),
        });
    }

    /// Open a popup to choose the reasoning effort (stage 2) for the given model.
    /// Kept for compatibility with callers that directly open the reasoning popup.
    pub(crate) fn open_reasoning_popup(&mut self, preset: ModelPreset) {
        let default_effort = preset.default_reasoning_effort;
        let supported = preset.supported_reasoning_efforts;
        let in_plan_mode =
            self.collaboration_modes_enabled() && self.active_mode_kind() == ModeKind::Plan;

        let warn_effort = if supported
            .iter()
            .any(|option| option.effort == ReasoningEffortConfig::XHigh)
        {
            Some(ReasoningEffortConfig::XHigh)
        } else if supported
            .iter()
            .any(|option| option.effort == ReasoningEffortConfig::High)
        {
            Some(ReasoningEffortConfig::High)
        } else {
            None
        };
        let warning_text = warn_effort.as_ref().map(|effort| {
            let effort_label = Self::reasoning_effort_label(effort);
            format!("⚠ {effort_label} reasoning effort can quickly consume Plus plan rate limits.")
        });
        let warn_for_model = preset.model.starts_with("gpt-5.1-ody")
            || preset.model.starts_with("gpt-5.1-ody-max")
            || preset.model.starts_with("gpt-5.2");

        let mut choices: Vec<ReasoningEffortConfig> = supported
            .iter()
            .map(|option| option.effort.clone())
            .collect();
        if choices.is_empty() {
            choices.push(default_effort.clone());
        }

        if choices.len() == 1 {
            let selected_effort = choices.first().cloned();
            let selected_model = preset.model;
            if self
                .should_prompt_plan_mode_reasoning_scope(&selected_model, selected_effort.clone())
            {
                self.app_event_tx
                    .send(AppEvent::OpenPlanReasoningScopePrompt {
                        model: selected_model,
                        effort: selected_effort,
                    });
            } else {
                self.apply_model_and_effort(selected_model, selected_effort);
            }
            return;
        }

        let default_choice = choices
            .contains(&default_effort)
            .then(|| default_effort.clone())
            .or_else(|| choices.first().cloned())
            .or(Some(default_effort));

        let model_slug = preset.model.to_string();
        let is_current_model = self.current_model() == preset.model.as_str();
        let highlight_choice = if is_current_model {
            if in_plan_mode {
                self.config
                    .plan_mode_reasoning_effort
                    .clone()
                    .or_else(|| self.effective_reasoning_effort())
            } else {
                self.effective_reasoning_effort()
            }
        } else {
            default_choice.clone()
        };
        let selection_choice = highlight_choice.clone().or_else(|| default_choice.clone());
        let initial_selected_idx = choices
            .iter()
            .position(|choice| Some(choice) == selection_choice.as_ref());
        let mut items: Vec<SelectionItem> = Vec::new();
        for choice in choices.iter() {
            let effort = choice.clone();
            let mut effort_label = Self::reasoning_effort_label(&effort);
            if Some(choice) == default_choice.as_ref() {
                effort_label.push_str(" (default)");
            }

            let description = supported
                .iter()
                .find(|option| option.effort == effort)
                .map(|option| option.description.to_string())
                .filter(|text| !text.is_empty());

            let show_warning = warn_for_model && warn_effort.as_ref() == Some(&effort);
            let selected_description = if show_warning {
                warning_text.as_ref().map(|warning_message| {
                    description.as_ref().map_or_else(
                        || warning_message.clone(),
                        |d| format!("{d}\n{warning_message}"),
                    )
                })
            } else {
                None
            };

            let model_for_action = model_slug.clone();
            let choice_effort = Some(effort);
            let should_prompt_plan_mode_scope = self.should_prompt_plan_mode_reasoning_scope(
                model_slug.as_str(),
                choice_effort.clone(),
            );
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                if should_prompt_plan_mode_scope {
                    tx.send(AppEvent::OpenPlanReasoningScopePrompt {
                        model: model_for_action.clone(),
                        effort: choice_effort.clone(),
                    });
                } else {
                    tx.send(AppEvent::UpdateModel(model_for_action.clone()));
                    tx.send(AppEvent::UpdateReasoningEffort(choice_effort.clone()));
                    tx.send(AppEvent::PersistModelSelection {
                        model: model_for_action.clone(),
                        effort: choice_effort.clone(),
                    });
                }
            })];

            items.push(SelectionItem {
                name: effort_label,
                description,
                selected_description,
                is_current: is_current_model && Some(choice) == highlight_choice.as_ref(),
                actions,
                dismiss_on_select: true,
                ..Default::default()
            });
        }

        let mut header = ColumnRenderable::new();
        header.push(Line::from(
            format!("Select Reasoning Level for {model_slug}").bold(),
        ));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            header: Box::new(header),
            footer_hint: Some(Line::from(
                "↑↓ to navigate · enter to select · esc to cancel",
            )),
            items,
            initial_selected_idx,
            ..Default::default()
        });
    }

    pub(super) fn reasoning_effort_label(effort: &ReasoningEffortConfig) -> String {
        match effort {
            ReasoningEffortConfig::None => "None".to_string(),
            ReasoningEffortConfig::Minimal => "Minimal".to_string(),
            ReasoningEffortConfig::Low => "Low".to_string(),
            ReasoningEffortConfig::Medium => "Medium".to_string(),
            ReasoningEffortConfig::High => "High".to_string(),
            ReasoningEffortConfig::XHigh => "Extra high".to_string(),
            ReasoningEffortConfig::Ultra => "Ultra".to_string(),
            ReasoningEffortConfig::Custom(value) => value.clone(),
        }
    }

    pub(super) fn reasoning_effort_sentence_label(effort: &ReasoningEffortConfig) -> String {
        match effort {
            ReasoningEffortConfig::Custom(value) => value.clone(),
            effort => Self::reasoning_effort_label(effort).to_lowercase(),
        }
    }

    pub(super) fn apply_model_and_effort_without_persist(
        &self,
        model: String,
        effort: Option<ReasoningEffortConfig>,
    ) {
        self.app_event_tx.send(AppEvent::UpdateModel(model));
        self.app_event_tx
            .send(AppEvent::UpdateReasoningEffort(effort));
    }

    fn apply_model_and_effort(&self, model: String, effort: Option<ReasoningEffortConfig>) {
        self.apply_model_and_effort_without_persist(model.clone(), effort.clone());
        self.app_event_tx
            .send(AppEvent::PersistModelSelection { model, effort });
    }
}
