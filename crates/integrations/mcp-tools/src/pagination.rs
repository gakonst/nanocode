use std::{collections::HashSet, future::Future, time::Duration};

const MAX_MCP_CATALOG_PAGES: usize = 100;
const MAX_MCP_CATALOG_ITEMS: usize = 2_048;
const MAX_MCP_PAGINATION_CURSOR_BYTES: usize = 64 * 1024;
const DEFAULT_MCP_PAGINATION_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) async fn collect_paginated<T, F, Fut>(
    method: &str,
    mut fetch: F,
) -> Result<Vec<T>, String>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: Future<Output = Result<(Vec<T>, Option<String>), String>>,
{
    let collect = async {
        let mut collected = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen_cursors = HashSet::new();

        for _ in 0..MAX_MCP_CATALOG_PAGES {
            let (items, next_cursor) = fetch(cursor.clone()).await?;
            if items.len() > MAX_MCP_CATALOG_ITEMS.saturating_sub(collected.len()) {
                return Err(format!(
                    "{method} exceeded the catalog limit of {MAX_MCP_CATALOG_ITEMS} items"
                ));
            }
            collected.extend(items);

            let Some(next_cursor) = next_cursor else {
                return Ok(collected);
            };
            if next_cursor.len() > MAX_MCP_PAGINATION_CURSOR_BYTES {
                return Err(format!(
                    "{method} returned a pagination cursor exceeding {MAX_MCP_PAGINATION_CURSOR_BYTES} bytes"
                ));
            }
            if !seen_cursors.insert(next_cursor.clone()) {
                return Err(format!("{method} returned a repeated pagination cursor"));
            }
            cursor = Some(next_cursor);
        }

        Err(format!(
            "{method} exceeded the pagination limit of {MAX_MCP_CATALOG_PAGES} pages"
        ))
    };

    let timeout = DEFAULT_MCP_PAGINATION_TIMEOUT;
    nanocode_platform::timeout(timeout, collect)
        .await
        .map_err(|_| format!("{method} pagination timed out after {timeout:?}"))?
}
