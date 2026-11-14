use std::collections::HashMap;

use super::*;
use crate::client::responses::FunctionOutputContent;
use nanocode_durability::{agent_store_error, base64_data_url};
use nanocode_image::ImageWindow;

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

fn request_images(model: Model) -> ImageWindow {
    match model {
        Model::Glm53 | Model::Kimi | Model::Mimo | Model::Custom(_)
            if model.as_str().starts_with("@cf/") =>
        {
            ImageWindow::WORKERS_AI_GATEWAY
        }
        Model::Glm53 | Model::Kimi | Model::Mimo | Model::Custom(_) => ImageWindow::GATEWAY,
        _ => ImageWindow::CODEX,
    }
}

fn bound(limit: ImageWindow, image: &str) -> Option<Rewrite> {
    let (_, data) = base64_data_url(image)?;
    Some(match limit.bound(data)? {
        Ok((data, media_type)) => {
            Rewrite::Inline(format!("data:{media_type};base64,{data}").into_boxed_str())
        }
        Err(text) => Rewrite::Omit(text.to_owned()),
    })
}

pub(super) fn evict_images(history: &RequestHistory, model: Model) -> RequestHistory {
    let limit = request_images(model);
    let full = history.full();
    let (_, start) = full.images_before(full.len());
    let (sent_before, _) = full.images_before(history.incremental_start());
    let Some(start) = start else {
        return history.clone();
    };
    let mut bounded: Vec<_> = full
        .iter_from(start)
        .flat_map(images)
        .map(|image| (image.len(), bound(limit, image)))
        .collect();
    let sizes: Vec<usize> = bounded
        .iter()
        .map(|(size, bounded)| match bounded {
            Some(Rewrite::Inline(url)) => url.len(),
            Some(Rewrite::Omit(_)) => 0,
            None => *size,
        })
        .collect();
    let evicted = limit.evicted_sizes(&sizes);
    let mut ordinal = 0;
    let mut changed = false;
    let replacement = full
        .iter_from(start)
        .cloned()
        .map(|mut item| {
            rewrite(&mut item, |image| {
                ordinal += 1;
                let rewrite = if ordinal <= evicted {
                    Some(Rewrite::Omit(limit.omitted(image)))
                } else {
                    bounded[ordinal - 1].1.take()
                };
                changed |= rewrite.is_some();
                rewrite
            });
            item
        })
        .collect();
    if !changed {
        return history.clone();
    }
    let continued = limit.evicted_sizes(&sizes[..sent_before.min(sizes.len())]) == evicted;
    let mut sent = history.clone();
    sent.replace_from(start, replacement, continued);
    sent
}

pub(super) fn omit_images(history: &RequestHistory) -> Option<RequestHistory> {
    let full = history.full();
    let start = full.images_before(full.len()).1?;
    let replacement = full
        .iter_from(start)
        .cloned()
        .map(|mut item| {
            rewrite(&mut item, |_| {
                Some(Rewrite::Omit(
                    "[image omitted after the provider rejected its data]".to_owned(),
                ))
            });
            item
        })
        .collect();
    let mut sent = history.clone();
    sent.replace_from(start, replacement, false);
    Some(sent)
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
