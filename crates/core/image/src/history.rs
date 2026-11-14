use nanocode_durability_core::{BlobUrl, StoreError};
use nanocode_types_core::{
    PromptInput, UserInput,
    responses::ContentItem,
    tools::{ToolOutputBody, ToolOutputContent},
};

pub use nanocode_durability_core::SharedBlobStore;

#[derive(Clone, Default)]
pub struct ImageHistory {
    blobs: Option<SharedBlobStore>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageWindow {
    images: Option<usize>,
    bytes: Option<usize>,
}

impl std::fmt::Debug for ImageHistory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImageHistory")
            .field("blob_store", &self.blobs.is_some())
            .finish()
    }
}

impl ImageWindow {
    pub const CLAUDE: Self = Self::images(100);
    pub const CLAUDE_LONG_CONTEXT: Self = Self::images(600);
    pub const CODEX: Self = Self::images(500);
    pub const WORKERS_AI: Self = Self::images(8);
    pub const GATEWAY: Self = Self {
        images: None,
        bytes: Some(2_641_785),
    };
    pub const WORKERS_AI_GATEWAY: Self = Self {
        images: Some(8),
        bytes: Some(2_641_785),
    };
    const IMAGES_PER_BUDGET: usize = 8;

    const fn images(images: usize) -> Self {
        Self {
            images: Some(images),
            bytes: None,
        }
    }

    #[must_use]
    pub fn bound(self, data: &str) -> Option<Result<(String, &'static str), &'static str>> {
        let budget = self.bytes? / Self::IMAGES_PER_BUDGET;
        (data.len() > budget).then(|| crate::bound_base64(data, budget))
    }

    #[must_use]
    pub fn evicted(self, images: usize) -> usize {
        let Some(limit) = self.images.filter(|limit| images > *limit) else {
            return 0;
        };
        let batch = limit.div_ceil(2).max(1);
        ((images - limit).div_ceil(batch) * batch).min(images)
    }

    #[must_use]
    pub fn evicted_sizes(self, sizes: &[usize]) -> usize {
        let mut evicted = 0;
        let mut bytes = 0;
        for (index, size) in sizes.iter().enumerate() {
            bytes += size;
            let budget = self.bytes.filter(|budget| bytes > *budget).map(|budget| budget / 2);
            while evicted < self.evicted(index + 1) || budget.is_some_and(|budget| bytes > budget) {
                bytes -= sizes[evicted];
                evicted += 1;
            }
        }
        evicted
    }

    #[must_use]
    pub fn omitted(self, reference: &str) -> String {
        let reference = reference
            .strip_prefix("data:")
            .and_then(|data| data.split([';', ',']).next())
            .map_or_else(|| reference.to_owned(), |media| format!("inline {media}"));
        match self.images {
            Some(images) => format!(
                "[image omitted from this request, which keeps only the newest {images} images: {reference}]"
            ),
            None => format!(
                "[image omitted from this request, which keeps only the newest images that fit its size budget: {reference}]"
            ),
        }
    }
}

impl ImageHistory {
    #[must_use]
    pub fn with_blob_store(mut self, store: SharedBlobStore) -> Self {
        self.blobs = Some(store);
        self
    }

    pub async fn inline(&self, url: &str) -> Result<Option<String>, StoreError> {
        let Some(blob) = BlobUrl::parse(url) else {
            return Ok(None);
        };
        let Some(store) = &self.blobs else {
            return Err(StoreError::Backend(format!(
                "image {url} needs a blob store, but none is configured"
            )));
        };
        blob.load_data_url(store.as_ref()).await.map(Some)
    }

    pub async fn store(&self, url: &str) -> Result<Option<String>, StoreError> {
        let Some(store) = &self.blobs else {
            return Ok(None);
        };
        Ok(BlobUrl::store_data_url(store.as_ref(), url)
            .await?
            .map(|blob| blob.to_string()))
    }

    pub async fn store_prompt(&self, input: &mut PromptInput) -> Result<(), StoreError> {
        let PromptInput::Content(items) = input else {
            return Ok(());
        };
        for item in items {
            if let UserInput::Image { image_url, .. } = item
                && let Some(blob) = self.store(image_url).await?
            {
                *image_url = blob;
            }
        }
        Ok(())
    }

    pub async fn load_prompt(&self, input: &mut PromptInput) -> Result<(), StoreError> {
        let PromptInput::Content(items) = input else {
            return Ok(());
        };
        for item in items {
            let UserInput::Image { image_url, .. } = item else {
                continue;
            };
            if !BlobUrl::is_blob_url(image_url) {
                continue;
            }
            if self.blobs.is_none() {
                *item = UserInput::Text {
                    text: format!(
                        "image content omitted because no blob store is configured to read {image_url}"
                    ),
                };
                continue;
            }
            if let Some(data) = self.inline(image_url).await? {
                *image_url = data;
            }
        }
        Ok(())
    }

    pub async fn prepare_user_input(
        &self,
        input: &PromptInput,
    ) -> Result<Vec<ContentItem>, StoreError> {
        let mut content = if has_blob_image(input) {
            let mut loaded = input.clone();
            self.load_prompt(&mut loaded).await?;
            crate::prepare_user_input(&loaded).await
        } else {
            crate::prepare_user_input(input).await
        };
        for item in &mut content {
            if let ContentItem::InputImage { image_url, .. } = item
                && let Some(blob) = self.store(image_url).await?
            {
                *image_url = blob.into_boxed_str();
            }
        }
        Ok(content)
    }

    pub async fn prepare_output_images(
        &self,
        output: &mut ToolOutputBody,
    ) -> Result<(), StoreError> {
        crate::prepare_output_images(output).await;
        let ToolOutputBody::Content(content) = output else {
            return Ok(());
        };
        for item in content {
            if let ToolOutputContent::InputImage { image_url, .. } = item
                && let Some(blob) = self.store(image_url).await?
            {
                *image_url = blob;
            }
        }
        Ok(())
    }
}

fn has_blob_image(input: &PromptInput) -> bool {
    let PromptInput::Content(items) = input else {
        return false;
    };
    items.iter().any(
        |item| matches!(item, UserInput::Image { image_url, .. } if BlobUrl::is_blob_url(image_url)),
    )
}
