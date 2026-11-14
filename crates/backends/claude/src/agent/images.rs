use std::collections::HashMap;

use crate::{
    ContentBlock, ToolResultContent,
    prompt::{IMAGE_URL_UNSUPPORTED, ImageResolution, prepare_base64},
};
use nanocode_agent_core::NanocodeError;
use nanocode_durability::{BlobUrl, agent_store_error, base64_data_url};
use nanocode_image::{ImageHistory, ImageWindow};
use serde_json::{Value, json};

use super::history::HistoryMessage;

pub(super) async fn prepare_tool_images(
    content: &mut ToolResultContent,
    resolution: ImageResolution,
    images: &ImageHistory,
) -> Result<(), NanocodeError> {
    let ToolResultContent::Blocks(blocks) = content else {
        return Ok(());
    };
    for block in blocks.iter_mut().filter(|block| block["type"] == "image") {
        match block["source"]["type"].as_str() {
            Some("url") if is_blob(&block["source"]) => {}
            Some("url") => *block = json!({"type": "text", "text": IMAGE_URL_UNSUPPORTED}),
            Some("base64") => {
                let data = match block["source"]["data"].take() {
                    Value::String(data) => data,
                    _ => String::new(),
                };
                match prepare_base64(&data, resolution).await {
                    Ok((data, media_type)) => {
                        let url = format!("data:{media_type};base64,{data}");
                        block["source"] = match images
                            .store(&url)
                            .await
                            .map_err(agent_store_error)?
                        {
                            Some(blob) => json!({"type": "url", "url": blob}),
                            None => {
                                json!({"type": "base64", "media_type": media_type, "data": data})
                            }
                        };
                    }
                    Err(omission) => *block = json!({"type": "text", "text": omission}),
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn is_blob(source: &Value) -> bool {
    source["url"].as_str().is_some_and(BlobUrl::is_blob_url)
}

fn reference(source: &Value) -> String {
    match source["type"].as_str() {
        Some("base64") => format!("data:{}", source["media_type"].as_str().unwrap_or("image")),
        Some("url") => source["url"].as_str().unwrap_or_default().to_owned(),
        Some("file") => source["file_id"].as_str().unwrap_or_default().to_owned(),
        _ => source.to_string(),
    }
}

fn image_sources(message: &HistoryMessage) -> impl Iterator<Item = &Value> {
    message.content.iter().flat_map(|block| match block {
        ContentBlock::Image { source, .. } => vec![source],
        ContentBlock::ToolResult {
            content: ToolResultContent::Blocks(blocks),
            ..
        } => blocks
            .iter()
            .filter(|block| block["type"] == "image")
            .map(|block| &block["source"])
            .collect(),
        _ => Vec::new(),
    })
}

enum Rewrite {
    Omit(String),
    Inline(Value),
}

fn rewrite(message: &mut HistoryMessage, mut replace: impl FnMut(&Value) -> Option<Rewrite>) {
    let replacements: Vec<_> = image_sources(message).map(&mut replace).collect();
    if replacements.iter().all(Option::is_none) {
        return;
    }
    let mut replacements = replacements.into_iter();
    for block in &mut message.make_mut().content {
        match block {
            ContentBlock::Image { source, extra } => match replacements.next().flatten() {
                Some(Rewrite::Omit(text)) => {
                    *block = ContentBlock::Text {
                        text,
                        extra: std::mem::take(extra),
                    };
                }
                Some(Rewrite::Inline(inline)) => *source = inline,
                None => {}
            },
            ContentBlock::ToolResult {
                content: ToolResultContent::Blocks(blocks),
                ..
            } => {
                for block in blocks.iter_mut().filter(|block| block["type"] == "image") {
                    match replacements.next().flatten() {
                        Some(Rewrite::Omit(text)) => {
                            let mut text = json!({"type": "text", "text": text});
                            if let Some(control) = block.get("cache_control") {
                                text["cache_control"] = control.clone();
                            }
                            *block = text;
                        }
                        Some(Rewrite::Inline(inline)) => block["source"] = inline,
                        None => {}
                    }
                }
            }
            _ => {}
        }
    }
}

pub(super) fn omit_images(messages: &mut [HistoryMessage]) -> bool {
    let mut omitted = false;
    for message in messages {
        rewrite(message, |_| {
            omitted = true;
            Some(Rewrite::Omit(
                "[image omitted after the provider rejected its data]".to_owned(),
            ))
        });
    }
    omitted
}

pub(super) fn evict_images(
    window: ImageWindow,
    mut messages: Vec<HistoryMessage>,
) -> (Vec<HistoryMessage>, [Option<usize>; 2]) {
    let counts: Vec<usize> = messages
        .iter()
        .map(|message| message.image_count(|message| image_sources(message).count()))
        .collect();
    let total: usize = counts.iter().sum();
    let evicted = window.evicted(total);
    let earlier = window.evicted(
        total
            - counts
                .iter()
                .rev()
                .find(|count| **count > 0)
                .copied()
                .unwrap_or_default(),
    );
    let holder = |skipped: usize| {
        let mut seen = 0;
        counts.iter().position(|count| {
            seen += count;
            seen > skipped
        })
    };
    let kept = [
        (earlier < evicted).then(|| holder(earlier)).flatten(),
        holder(evicted),
    ];
    let mut ordinal = 0;
    for message in &mut messages {
        if ordinal >= evicted {
            break;
        }
        rewrite(message, |source| {
            ordinal += 1;
            (ordinal <= evicted).then(|| Rewrite::Omit(window.omitted(&reference(source))))
        });
    }
    (messages, kept)
}

pub(super) async fn inline_images(
    images: &ImageHistory,
    messages: &[HistoryMessage],
) -> Result<Option<Vec<HistoryMessage>>, NanocodeError> {
    let mut loaded = HashMap::new();
    for source in messages
        .iter()
        .filter(|message| message.image_count(|message| image_sources(message).count()) > 0)
        .flat_map(image_sources)
    {
        let Some(url) = source["url"].as_str().filter(|_| is_blob(source)) else {
            continue;
        };
        if loaded.contains_key(url) {
            continue;
        }
        let data = images
            .inline(url)
            .await
            .map_err(agent_store_error)?
            .unwrap_or_default();
        let (media_type, data) = base64_data_url(&data).unwrap_or_default();
        loaded.insert(
            url.to_owned(),
            json!({"type": "base64", "media_type": media_type, "data": data}),
        );
    }
    if loaded.is_empty() {
        return Ok(None);
    }
    let mut messages = messages.to_vec();
    for message in &mut messages {
        rewrite(message, |source| {
            source["url"]
                .as_str()
                .and_then(|url| loaded.get(url))
                .cloned()
                .map(Rewrite::Inline)
        });
    }
    Ok(Some(messages))
}
