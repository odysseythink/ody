//! Real-provider smoke test for the unified `ChatProvider` path.
//!
//! Reads `~/.ody-code/config.toml`, picks one model per configured Chat-provider
//! family (kimi/deepseek/glm), sends a tiny completion request through the
//! `ody_model_provider::ChatProvider` adapter, and reports whether a non-error
//! text response was received.
//!
//! Run with:
//!   cargo run -p ody-cli --example chat_provider_smoke
//!
//! To test only specific providers:
//!   CHAT_PROVIDER_SMOKE_FILTER=kimi,deepseek cargo run -p ody-cli --example chat_provider_smoke

use std::collections::HashMap;
use std::path::PathBuf;

use futures::StreamExt;
use ody_model_provider::{ChatRequest, ContentPart, Message, Role, create_model_provider_with_id};
use ody_model_provider_info::{ModelProviderInfo, ProviderCapabilities, WireApi};

#[derive(Debug, serde::Deserialize)]
struct UserConfig {
    #[serde(default)]
    providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    models: HashMap<String, ModelConfig>,
    #[serde(default)]
    default_model: Option<String>,
}

#[derive(Debug, serde::Deserialize, Clone)]
struct ProviderConfig {
    r#type: String,
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    base_url: String,
}

#[derive(Debug, serde::Deserialize)]
struct ModelConfig {
    provider: String,
    model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ProviderFamily {
    Kimi,
    DeepSeek,
    Glm,
}

impl ProviderFamily {
    fn from_provider_type(t: &str) -> Option<Self> {
        match t {
            "kimi" => Some(ProviderFamily::Kimi),
            "deepseek" => Some(ProviderFamily::DeepSeek),
            "glm" => Some(ProviderFamily::Glm),
            _ => None,
        }
    }

    fn wire_api(&self) -> WireApi {
        WireApi::Chat
    }

    fn display_name(&self) -> &'static str {
        match self {
            ProviderFamily::Kimi => "Kimi",
            ProviderFamily::DeepSeek => "DeepSeek",
            ProviderFamily::Glm => "GLM",
        }
    }
}

#[tokio::main]
async fn main() {
    let home = std::env::var("HOME").expect("HOME env var required");
    let config_path = PathBuf::from(home).join(".ody-code").join("config.toml");
    let config_text = std::fs::read_to_string(&config_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", config_path.display(), e));
    let config: UserConfig = toml::from_str(&config_text)
        .unwrap_or_else(|e| panic!("failed to parse {}: {}", config_path.display(), e));

    let filter = std::env::var("CHAT_PROVIDER_SMOKE_FILTER")
        .ok()
        .map(|s| s.split(',').map(|s| s.trim().to_lowercase()).collect::<Vec<_>>());

    // Parse default model provider so we can prefer it within its family.
    let default_provider = config.default_model.as_ref().and_then(|dm| dm.split_once('/').map(|(p, _)| p.to_string()));

    // Collect candidates per provider family.
    let mut candidates: HashMap<ProviderFamily, Vec<(String, String, ProviderConfig)>> = HashMap::new();
    for (model_key, model_cfg) in &config.models {
        let provider_cfg = match config.providers.get(&model_cfg.provider) {
            Some(p) => p,
            None => {
                eprintln!(
                    "model {} references unknown provider {}; skipping",
                    model_key, model_cfg.provider
                );
                continue;
            }
        };
        if provider_cfg.api_key.is_empty() {
            eprintln!(
                "provider {} has no api_key; skipping {}",
                model_cfg.provider, model_key
            );
            continue;
        }
        let family = match ProviderFamily::from_provider_type(&provider_cfg.r#type) {
            Some(f) => f,
            None => {
                eprintln!(
                    "unsupported provider type {}; skipping {}",
                    provider_cfg.r#type, model_key
                );
                continue;
            }
        };
        if let Some(ref f) = filter {
            if !f.contains(&family.display_name().to_lowercase()) {
                continue;
            }
        }
        candidates
            .entry(family)
            .or_default()
            .push((model_cfg.provider.clone(), model_cfg.model.clone(), provider_cfg.clone()));
    }

    // Prefer the default-model provider when it belongs to a testable family.
    let mut selected: HashMap<ProviderFamily, (String, String, ProviderConfig)> = HashMap::new();
    for (family, mut list) in candidates {
        if let Some(ref dp) = default_provider {
            if let Some(pos) = list.iter().position(|(pid, _, _)| pid == dp) {
                let chosen = list.remove(pos);
                selected.insert(family, chosen);
                continue;
            }
        }
        if let Some(first) = list.into_iter().next() {
            selected.insert(family, first);
        }
    }

    if selected.is_empty() {
        println!("No testable Chat providers found in config.");
        std::process::exit(1);
    }

    let mut results: Vec<(String, anyhow::Result<String>)> = Vec::new();
    for (family, (provider_id, model_name, provider_cfg)) in &selected {
        let label = format!("{} ({}/{})", family.display_name(), provider_id, model_name);
        let result = smoke_one(provider_id, model_name, provider_cfg).await;
        results.push((label, result));
    }

    let mut all_ok = true;
    for (label, result) in results {
        match result {
            Ok(text) => println!("{}: OK -> {:?}", label, text.trim()),
            Err(e) => {
                println!("{}: FAIL -> {}", label, e);
                all_ok = false;
            }
        }
    }

    if !all_ok {
        std::process::exit(1);
    }
}

async fn smoke_one(
    provider_id: &str,
    model_name: &str,
    provider_cfg: &ProviderConfig,
) -> anyhow::Result<String> {
    let family = ProviderFamily::from_provider_type(&provider_cfg.r#type)
        .ok_or_else(|| anyhow::anyhow!("unsupported provider type {}", provider_cfg.r#type))?;

    let info = ModelProviderInfo {
        name: family.display_name().to_string(),
        base_url: Some(provider_cfg.base_url.clone()),
        experimental_bearer_token: Some(provider_cfg.api_key.clone()),
        wire_api: family.wire_api(),
        request_max_retries: Some(0),
        stream_max_retries: Some(0),
        capabilities: ProviderCapabilities::default(),
        ..Default::default()
    };

    let provider = create_model_provider_with_id(provider_id, info, None);
    let chat = provider
        .chat_provider()
        .await
        .map_err(|e| anyhow::anyhow!("chat_provider: {e}"))?;

    let request = ChatRequest {
        model: model_name.to_string(),
        messages: vec![Message {
            role: Role::User,
            content: vec![ContentPart::Text(
                "Reply with the single word 'pong' and nothing else.".to_string(),
            )],
            tool_calls: vec![],
            tool_call_id: None,
        }],
        tools: vec![],
        thinking_effort: ody_model_provider::ThinkingEffort::Off,
        max_tokens: Some(20),
        temperature: None,
        top_p: None,
        stop: vec![],
        ..Default::default()
    };

    let debug = std::env::var("CHAT_PROVIDER_SMOKE_DEBUG").is_ok();
    let mut stream = chat.chat(request).await.map_err(|e| anyhow::anyhow!("chat: {e}"))?;
    let mut text = String::new();
    let mut reasoning = String::new();
    while let Some(result) = stream.next().await {
        if debug {
            eprintln!("  event: {:?}", result);
        }
        match result {
            Ok(ody_model_provider::ChatEvent::ContentPart(ContentPart::Text(t))) => {
                text.push_str(&t)
            }
            Ok(ody_model_provider::ChatEvent::ReasoningPart(r)) => reasoning.push_str(&r),
            Ok(ody_model_provider::ChatEvent::Error(e)) => {
                return Err(anyhow::anyhow!("stream error: {e}"))
            }
            Err(e) => return Err(anyhow::anyhow!("stream item error: {e}")),
            _ => {}
        }
    }

    if debug {
        eprintln!("  accumulated text: {:?}, reasoning: {:?}", text, reasoning);
    }

    let combined = format!("{}{}", text, reasoning);
    if combined.trim().is_empty() {
        return Err(anyhow::anyhow!("empty response"));
    }

    Ok(combined)
}
