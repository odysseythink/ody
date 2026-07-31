use anyhow::Context;
use anyhow::Result;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_ody::test_ody;
use core_test_support::wait_for_event;
use ody_protocol::model_metadata::InputModality;
use ody_protocol::models::ImageDetail;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::Op;
use ody_protocol::user_input::UserInput;
use pretty_assertions::assert_eq;
use serde_json::Value;

const RESPONSES_LITE_HEADER: &str = "x-odysseythink-internal-ody-responses-lite";

fn configure_image_capable_model(model_info: &mut ody_protocol::model_metadata::ModelInfo) {
    model_info.input_modalities = vec![InputModality::Text, InputModality::Image];
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_lite_prepares_images() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_completed("resp-1"),
        ]),
    )
    .await;
    let image_url = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/iZk9HQAAAABJRU5ErJggg==";
    let remote_image_url = "https://example.com/image.png";
    let mut builder = test_ody().with_model_info_override("k3", |model_info| {
        model_info.use_responses_lite = true;
        configure_image_capable_model(model_info);
    });
    let test = builder.build(&server).await?;

    test.ody
        .submit(Op::UserInput {
            items: vec![
                UserInput::Image {
                    image_url: image_url.to_string(),
                    detail: Some(ImageDetail::Original),
                },
                UserInput::Image {
                    image_url: remote_image_url.to_string(),
                    detail: Some(ImageDetail::High),
                },
            ],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;
    wait_for_event(&test.ody, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let request = response_mock.single_request();
    let user_content = request
        .input()
        .into_iter()
        .rev()
        .find(|item| item.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|item| item.get("content").and_then(Value::as_array).cloned())
        .context("request should contain user content")?;
    assert_eq!(
        user_content,
        vec![
            serde_json::json!({
                "type": "input_image",
                "image_url": image_url
            }),
            serde_json::json!({
                "type": "input_text",
                "text": "image content omitted because remote image URLs are not supported"
            }),
        ]
    );
    assert!(!request.body_json().to_string().contains(remote_image_url));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn responses_lite_compact_request_uses_lite_transport_contract() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = responses::start_mock_server().await;
    let response_mock = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            responses::ev_response_created("resp-1"),
            responses::ev_completed("resp-1"),
        ]),
    )
    .await;
    let compact_mock =
        responses::mount_compact_json_once(&server, serde_json::json!({ "output": [] })).await;

    let mut builder = test_ody()
        .with_model_info_override("k3", |model_info| {
            model_info.use_responses_lite = true;
            model_info.supports_parallel_tool_calls = true;
        })
        .with_config(|config| {
            let _ = config
                .features
                .disable(ody_features::Feature::RemoteCompactionV2);
        });
    let test = builder.build(&server).await?;

    test.submit_turn("Compact this conversation").await?;
    test.ody.submit(Op::Compact).await?;
    wait_for_event(&test.ody, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    response_mock.single_request();
    let compact_request = compact_mock.single_request();
    assert_eq!(
        compact_request.header(RESPONSES_LITE_HEADER).as_deref(),
        Some("true")
    );
    let compact_body = compact_request.body_json();
    assert_eq!(
        compact_body
            .get("reasoning")
            .and_then(|reasoning| reasoning.get("context"))
            .and_then(Value::as_str),
        Some("all_turns")
    );
    assert_eq!(
        compact_body.get("parallel_tool_calls"),
        Some(&Value::Bool(false))
    );

    Ok(())
}
