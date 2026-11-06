use super::*;
pub(super) fn model_visible_len(item: &ResponseItem) -> usize {
    match item {
        ResponseItem::Message { content, .. } => content
            .iter()
            .map(|part| match part {
                ContentItem::InputText { text } | ContentItem::OutputText { text, .. } => {
                    text.len()
                }
                ContentItem::InputImage { image_url, detail } => image_bytes(image_url, *detail),
                ContentItem::InputImageFile { .. } => RESIZED_IMAGE_BYTES_ESTIMATE,
                ContentItem::InputAudio { audio_url } => audio_bytes(audio_url),
            })
            .fold(0, usize::saturating_add),
        ResponseItem::AgentMessage {
            author,
            recipient,
            content,
            ..
        } => content
            .iter()
            .map(|part| match part {
                crate::client::responses::AgentMessageContent::InputText { text } => text.len(),
                crate::client::responses::AgentMessageContent::EncryptedContent {
                    encrypted_content,
                } => encrypted_content.len().saturating_mul(9).div_ceil(16),
            })
            .fold(
                author.len().saturating_add(recipient.len()),
                usize::saturating_add,
            ),
        ResponseItem::Reasoning {
            encrypted_content: Some(content),
            ..
        }
        | ResponseItem::ContextCompaction {
            encrypted_content: Some(content),
            ..
        }
        | ResponseItem::Compaction {
            encrypted_content: content,
            ..
        } => content
            .len()
            .saturating_mul(3)
            .checked_div(4)
            .unwrap_or(0)
            .saturating_sub(650),
        ResponseItem::FunctionCall {
            name,
            namespace,
            arguments: input,
            ..
        }
        | ResponseItem::CustomToolCall {
            name,
            namespace,
            input,
            ..
        } => name
            .len()
            .saturating_add(namespace.as_deref().unwrap_or("functions").len())
            .saturating_add(input.len()),
        ResponseItem::FunctionCallOutput {
            call_id, output, ..
        } => output_bytes(output).saturating_add(call_id.len()),
        ResponseItem::CustomToolCallOutput {
            call_id,
            name,
            output,
            ..
        } => output_bytes(output)
            .saturating_add(call_id.len())
            .saturating_add(name.as_deref().unwrap_or_default().len()),
        ResponseItem::AdditionalTools { tools, .. } => json_bytes(tools),
        ResponseItem::ToolSearchCall { arguments, .. } => json_bytes(arguments),
        ResponseItem::ToolSearchOutput { tools, .. } => json_bytes(tools),
        ResponseItem::LocalShellCall { action, .. } => json_bytes(action),
        ResponseItem::WebSearchCall { action, .. } => action.as_ref().map_or(0, json_bytes),
        ResponseItem::ImageGenerationCall {
            revised_prompt,
            result,
            ..
        } => revised_prompt
            .as_deref()
            .unwrap_or_default()
            .len()
            .saturating_add(if result.is_empty() {
                0
            } else {
                RESIZED_IMAGE_BYTES_ESTIMATE
            }),
        ResponseItem::Reasoning {
            encrypted_content: None,
            ..
        }
        | ResponseItem::ContextCompaction {
            encrypted_content: None,
            ..
        }
        | ResponseItem::ConfigurationUpdate { .. }
        | ResponseItem::CompactionTrigger { .. }
        | ResponseItem::Other(_) => 0,
    }
}
fn json_bytes(value: &(impl serde::Serialize + ?Sized)) -> usize {
    serde_json::to_vec(value).map_or(0, |bytes| bytes.len())
}
fn image_bytes(url: &str, detail: Option<ImageDetail>) -> usize {
    if detail == Some(ImageDetail::Original) {
        original_image_bytes_estimate(url).unwrap_or(RESIZED_IMAGE_BYTES_ESTIMATE)
    } else {
        RESIZED_IMAGE_BYTES_ESTIMATE
    }
}
fn output_bytes(output: &FunctionOutputBody) -> usize {
    match output {
        FunctionOutputBody::Text(text) => text.len(),
        FunctionOutputBody::Content(content) => content
            .iter()
            .map(|part| match part {
                FunctionOutputContent::InputText { text } => text.len(),
                FunctionOutputContent::InputImage { image_url, detail } => {
                    image_bytes(image_url, *detail)
                }
                FunctionOutputContent::InputImageFile { .. } => RESIZED_IMAGE_BYTES_ESTIMATE,
                FunctionOutputContent::InputAudio { audio_url } => audio_bytes(audio_url),
                FunctionOutputContent::EncryptedContent { encrypted_content } => {
                    encrypted_content.len().saturating_mul(9).div_ceil(16)
                }
            })
            .fold(0, usize::saturating_add),
    }
}

fn audio_bytes(url: &str) -> usize {
    nanocode_audio::estimate_audio_token_count(url).saturating_mul(APPROX_BYTES_PER_TOKEN)
}
