use ody_core::config::Config;

use serde::de::DeserializeOwned;
use std::time::Duration;

const OAI_PRODUCT_SKU_HEADER: &str = "OAI-Product-Sku";
const ODY_PRODUCT_SKU: &str = "ody";

/// Make a GET request to the ChatGPT backend API.
pub(crate) async fn chatgpt_get_request<T: DeserializeOwned>(
    config: &Config,
    path: String,
) -> anyhow::Result<T> {
    chatgpt_get_request_with_timeout(config, path, /*timeout*/ None).await
}

pub(crate) async fn chatgpt_get_request_with_timeout<T: DeserializeOwned>(
    config: &Config,
    path: String,
    timeout: Option<Duration>,
) -> anyhow::Result<T> {
    // The remote hosted plugin/Apps catalog config field this used to be sourced from has been
    // removed. Every caller of this function requires `uses_ody_backend()` auth, which can never
    // be true anymore, so this request path is unreachable.
    let _ = (config, path, timeout);
    anyhow::bail!("ChatGPT backend requests require Ody backend auth")
}
