use ody_features::FEATURES;
use ody_features::Feature;
use std::collections::BTreeMap;
use std::path::Path;

pub fn write_mock_responses_config_toml_simple(
    ody_home: &Path,
    server_uri: &str,
) -> std::io::Result<()> {
    let config_toml = ody_home.join("config.toml");
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"

model_provider = "mock_provider"

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#
        ),
    )
}

pub fn write_mock_responses_config_toml(
    ody_home: &Path,
    server_uri: &str,
    feature_flags: &BTreeMap<Feature, bool>,
    auto_compact_limit: i64,
    requires_odysseythink_auth: Option<bool>,
    model_provider_id: &str,
    compact_prompt: &str,
) -> std::io::Result<()> {
    // Phase 1: build the features block for config.toml.
    let mut features = BTreeMap::new();
    for (feature, enabled) in feature_flags {
        features.insert(*feature, *enabled);
    }
    let feature_entries = features
        .into_iter()
        .map(|(feature, enabled)| {
            let key = FEATURES
                .iter()
                .find(|spec| spec.id == feature)
                .map(|spec| spec.key)
                .expect("feature should have a config key");
            format!("{key} = {enabled}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    // Phase 2: build provider-specific config bits.
    let requires_line = match requires_odysseythink_auth {
        Some(true) => "requires_odysseythink_auth = true\n".to_string(),
        Some(false) | None => String::new(),
    };
    let provider_name = if matches!(requires_odysseythink_auth, Some(true)) {
        "OpenAI"
    } else {
        "Mock provider for test"
    };
    let provider_block = format!(
        r#"
[model_providers.{model_provider_id}]
name = "{provider_name}"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
supports_websockets = false
{requires_line}
"#
    );
    let odysseythink_base_url_line = if model_provider_id == "odysseythink" {
        format!("odysseythink_base_url = \"{server_uri}/v1\"\n")
    } else {
        String::new()
    };
    // Phase 3: write the final config file.
    let config_toml = ody_home.join("config.toml");
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"
compact_prompt = "{compact_prompt}"
model_auto_compact_token_limit = {auto_compact_limit}

model_provider = "{model_provider_id}"
{odysseythink_base_url_line}

[features]
{feature_entries}
{provider_block}
"#
        ),
    )
}

