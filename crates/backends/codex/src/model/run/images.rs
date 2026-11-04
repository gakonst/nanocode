use std::collections::HashMap;

use super::*;
use crate::client::responses::FunctionOutputContent;
use nanocode_durability::agent_store_error;

fn images(item: &ResponseItem) -> Vec<&str> {
    match item {
        ResponseItem::Message { content, .. } => content
            .iter()
            .filter_map(|part| match part {
                ContentItem::InputImage { image_url, .. } => Some(&**image_url),
                ContentItem::InputImageFile { file_id, .. } => Some(&**file_id),
                _ => None,
            })
            .collect(),
        ResponseItem::FunctionCallOutput {
            output: FunctionOutputBody::Content(content),
            ..
        }
        | ResponseItem::CustomToolCallOutput {
            output: FunctionOutputBody::Content(content),
            ..
        } => content
            .iter()
            .filter_map(|part| match part {
                FunctionOutputContent::InputImage { image_url, .. } => Some(&**image_url),
                FunctionOutputContent::InputImageFile { file_id, .. } => Some(&**file_id),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

enum Rewrite {
    Omit(String),
    Inline(Box<str>),
}

fn rewrite(item: &mut ResponseItem, mut replace: impl FnMut(&str) -> Option<Rewrite>) {
    match item {
        ResponseItem::Message { content, .. } => {
            for part in content {
                let replacement = match part {
                    ContentItem::InputImage { image_url, .. } => replace(image_url),
                    ContentItem::InputImageFile { file_id, .. } => replace(file_id),
                    _ => continue,
                };
                match (replacement, part) {
                    (Some(Rewrite::Omit(text)), part) => {
                        *part = ContentItem::InputText { text: text.into() };
                    }
                    (Some(Rewrite::Inline(url)), ContentItem::InputImage { image_url, .. }) => {
                        *image_url = url;
                    }
                    _ => {}
                }
            }
        }
        ResponseItem::FunctionCallOutput {
            output: FunctionOutputBody::Content(content),
            ..
        }
        | ResponseItem::CustomToolCallOutput {
            output: FunctionOutputBody::Content(content),
            ..
        } => {
            for part in content {
                let replacement = match part {
                    FunctionOutputContent::InputImage { image_url, .. } => replace(image_url),
                    FunctionOutputContent::InputImageFile { file_id, .. } => replace(file_id),
                    _ => continue,
                };
                match (replacement, part) {
                    (Some(Rewrite::Omit(text)), part) => {
                        *part = FunctionOutputContent::InputText { text: text.into() };
                    }
                    (
                        Some(Rewrite::Inline(url)),
                        FunctionOutputContent::InputImage { image_url, .. },
                    ) => *image_url = url,
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

pub(super) fn evict_images(limit: &ImageHistory, history: &RequestHistory) -> RequestHistory {
    let full = history.full();
    let (total, start) = full.images_before(full.len());
    let (sent_before, _) = full.images_before(history.incremental_start());
    let evicted = limit.evicted(total);
    let Some(start) = start.filter(|_| evicted > 0) else {
        return history.clone();
    };
    let mut ordinal = 0;
    let replacement = full
        .iter_from(start)
        .cloned()
        .map(|mut item| {
            rewrite(&mut item, |image| {
                ordinal += 1;
                (ordinal <= evicted).then(|| Rewrite::Omit(limit.omitted(image)))
            });
            item
        })
        .collect();
    let mut sent = history.clone();
    sent.replace_from(start, replacement, limit.evicted(sent_before) == evicted);
    sent
}

pub(super) async fn inline_images(
    limit: &ImageHistory,
    history: RequestHistory,
) -> Result<RequestHistory> {
    let full = history.full();
    let (_, Some(first)) = full.images_before(full.len()) else {
        return Ok(history);
    };
    let mut start = None;
    let mut loaded = HashMap::new();
    for (index, item) in full.iter_from(first).enumerate() {
        let index = first + index;
        for url in images(item) {
            if loaded.contains_key(url) {
                continue;
            }
            let Some(data) = limit.inline(url).await.map_err(agent_store_error)? else {
                continue;
            };
            start.get_or_insert(index);
            loaded.insert(url.to_owned(), data.into_boxed_str());
        }
    }
    let Some(start) = start else {
        return Ok(history);
    };
    let replacement = full
        .iter_from(start)
        .cloned()
        .map(|mut item| {
            rewrite(&mut item, |url| {
                loaded.get(url).cloned().map(Rewrite::Inline)
            });
            item
        })
        .collect();
    let mut sent = history.clone();
    sent.replace_from(start, replacement, true);
    Ok(sent)
}
