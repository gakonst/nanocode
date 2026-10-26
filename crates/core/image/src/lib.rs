use std::{
    borrow::Cow,
    collections::{HashMap, VecDeque},
    io::Cursor,
    path::Path,
    sync::{Arc, LazyLock},
};

use arc_swap::ArcSwap;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use image::{
    ColorType, DynamicImage, GenericImageView, ImageDecoder, ImageEncoder, ImageFormat,
    ImageReader, codecs::jpeg::JpegEncoder,
};
mod history;
mod metadata;
mod pam;

pub use history::{DEFAULT_INLINE_IMAGES, ImageHistory, SharedBlobStore};

pub use nanocode_types_core::ImageDetail;
use nanocode_types_core::{PromptInput, UserInput, responses::ContentItem};
use sha1::{Digest as _, Sha1};

use nanocode_types_core::tools::{ToolOutputBody, ToolOutputContent};

pub const IMAGE_PROCESSING_ERROR_PLACEHOLDER: &str =
    "image content omitted because it could not be processed";
const IMAGE_TOO_LARGE_PLACEHOLDER: &str =
    "image content omitted because it exceeded the supported size limit; use a smaller image";
const UNSUPPORTED_LOW_DETAIL_PLACEHOLDER: &str = "image content omitted because detail 'low' is not supported; use 'high', 'original', or 'auto'";
const REMOTE_IMAGE_URL_PLACEHOLDER: &str =
    "image content omitted because remote image URLs are not supported";

const DATA_URL_PREFIX: &str = "data:";
const PROMPT_IMAGE_PATCH_SIZE: u32 = 32;
const MAX_PROMPT_IMAGE_INPUT_BYTES: usize = 1024 * 1024 * 1024;

#[cfg(target_family = "wasm")]
const MAX_PROMPT_IMAGE_DECODE_BYTES: u64 = 40 * 1024 * 1024;
#[cfg(not(target_family = "wasm"))]
const MAX_PROMPT_IMAGE_DECODE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_IMAGE_CACHE_ENTRIES: usize = 32;
const MAX_IMAGE_CACHE_BYTES: usize = 64 * 1024 * 1024;

const HIGH_DETAIL_LIMITS: PromptImageResizeLimits = PromptImageResizeLimits {
    max_dimension: 2048,
    max_patches: 2_500,
};
const ORIGINAL_DETAIL_LIMITS: PromptImageResizeLimits = PromptImageResizeLimits {
    max_dimension: 6000,
    max_patches: 10_000,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct PromptImageResizeLimits {
    max_dimension: u32,
    max_patches: u32,
}

#[derive(Clone)]
struct EncodedImage {
    bytes: Arc<[u8]>,
    mime: &'static str,
}

impl EncodedImage {
    fn into_data_url(self) -> String {
        encode_data_url(self.mime, &self.bytes)
    }
}

fn encode_data_url(mime: &str, bytes: &[u8]) -> String {
    let mut url = String::with_capacity(
        "data:;base64,".len() + mime.len() + base64_simd::STANDARD.encoded_length(bytes.len()),
    );
    url.push_str("data:");
    url.push_str(mime);
    url.push_str(";base64,");
    base64_simd::STANDARD.encode_append(bytes, &mut url);
    url
}

struct ImageMetadata {
    icc_profile: Option<Vec<u8>>,
    exif: Option<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct ImageCacheKey {
    digest: [u8; 20],
    limits: PromptImageResizeLimits,
}

#[derive(Clone, Default)]
struct ImageCache {
    entries: HashMap<ImageCacheKey, EncodedImage>,
    order: VecDeque<ImageCacheKey>,
    bytes: usize,
}

impl ImageCache {
    fn get(&mut self, key: &ImageCacheKey) -> Option<EncodedImage> {
        let image = self.entries.get(key)?.clone();
        self.order.retain(|candidate| candidate != key);
        self.order.push_back(*key);
        Some(image)
    }

    fn insert(&mut self, key: ImageCacheKey, image: EncodedImage) {
        self.insert_with_limits(key, image, MAX_IMAGE_CACHE_ENTRIES, MAX_IMAGE_CACHE_BYTES);
    }

    fn insert_with_limits(
        &mut self,
        key: ImageCacheKey,
        image: EncodedImage,
        entry_capacity: usize,
        byte_capacity: usize,
    ) {
        if image.bytes.len() > byte_capacity {
            return;
        }
        if let Some(previous) = self.entries.remove(&key) {
            self.bytes = self.bytes.saturating_sub(previous.bytes.len());
            self.order.retain(|candidate| *candidate != key);
        }
        self.bytes = self.bytes.saturating_add(image.bytes.len());
        self.entries.insert(key, image);
        self.order.push_back(key);
        while self.entries.len() > entry_capacity || self.bytes > byte_capacity {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(evicted) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(evicted.bytes.len());
            }
        }
    }
}

static IMAGE_CACHE: LazyLock<ArcSwap<ImageCache>> = LazyLock::new(ArcSwap::default);

fn cached_image(key: &ImageCacheKey) -> Option<EncodedImage> {
    let mut image = None;
    IMAGE_CACHE.rcu(|cache| {
        let mut cache = ImageCache::clone(cache);
        image = cache.get(key);
        cache
    });
    image
}

fn cache_image(key: ImageCacheKey, image: EncodedImage) {
    IMAGE_CACHE.rcu(|cache| {
        let mut cache = ImageCache::clone(cache);
        cache.insert(key, image.clone());
        cache
    });
}

#[derive(Debug, thiserror::Error)]
enum ImagePreparationError {
    #[error("remote image URLs are not supported")]
    RemoteUrlUnsupported,
    #[error("image detail `low` is not supported")]
    UnsupportedLowDetail,
    #[error("image {representation} is too large ({size} bytes; max {max} bytes)")]
    ImageTooLarge {
        representation: &'static str,
        size: usize,
        max: usize,
    },
    #[error("{0}")]
    Processing(String),
}

impl ImagePreparationError {
    const fn placeholder(&self) -> &'static str {
        match self {
            Self::RemoteUrlUnsupported => REMOTE_IMAGE_URL_PLACEHOLDER,
            Self::UnsupportedLowDetail => UNSUPPORTED_LOW_DETAIL_PLACEHOLDER,
            Self::ImageTooLarge { .. } => IMAGE_TOO_LARGE_PLACEHOLDER,
            Self::Processing(_) => IMAGE_PROCESSING_ERROR_PLACEHOLDER,
        }
    }
}

#[allow(
    clippy::unused_async,
    reason = "WASM prepares images inline without a blocking pool"
)]
pub async fn prepare_output_images(output: &mut ToolOutputBody) {
    let ToolOutputBody::Content(content) = output else {
        return;
    };
    for item in content.iter_mut() {
        if let ToolOutputContent::InputImageFile { file_id, .. } = item
            && !nanocode_types_core::responses::valid_image_file_id(file_id)
        {
            *item = ToolOutputContent::InputText {
                text: "Image file reference is malformed".to_owned(),
            };
        }
    }
    if !content
        .iter()
        .any(|item| matches!(item, ToolOutputContent::InputImage { .. }))
    {
        return;
    }
    let content = std::mem::take(content);
    #[cfg(target_family = "wasm")]
    {
        *output = ToolOutputBody::Content(prepare_content(content));
        output.replace_invalid_image_envelopes();
    }
    #[cfg(not(target_family = "wasm"))]
    match tokio::task::spawn_blocking(move || prepare_content(content)).await {
        Ok(prepared) => {
            let ToolOutputBody::Content(output) = output else {
                return;
            };
            *output = prepared;
        }
        Err(error) => {
            tracing::warn!(%error, "failed to join image preparation task");
            *output = ToolOutputBody::Content(vec![ToolOutputContent::InputText {
                text: IMAGE_PROCESSING_ERROR_PLACEHOLDER.to_owned(),
            }]);
        }
    }
}

pub fn prepare_history_images(items: &mut [nanocode_types_core::responses::ResponseItem]) -> bool {
    use nanocode_types_core::responses::{FunctionOutputBody, FunctionOutputContent, ResponseItem};
    let mut changed = false;
    for item in items {
        match item {
            ResponseItem::Message { content, .. } => {
                for part in content {
                    if let ContentItem::InputImage { image_url, detail } = part {
                        let mut url = image_url.to_string();
                        match prepare_image(&mut url, detail.unwrap_or(ImageDetail::Auto)) {
                            Ok(()) => {
                                changed |= url != image_url.as_ref();
                                *image_url = url.into_boxed_str();
                            }
                            Err(error) => {
                                *part = ContentItem::input_text(error.placeholder());
                                changed = true;
                            }
                        }
                    }
                }
            }
            ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. } => {
                let FunctionOutputBody::Content(content) = output else {
                    continue;
                };
                for part in content {
                    if let FunctionOutputContent::InputImage { image_url, detail } = part {
                        let mut url = image_url.to_string();
                        match prepare_image(&mut url, detail.unwrap_or(ImageDetail::Auto)) {
                            Ok(()) => {
                                changed |= url != image_url.as_ref();
                                *image_url = url.into_boxed_str();
                            }
                            Err(error) => {
                                *part = FunctionOutputContent::InputText {
                                    text: error.placeholder().into(),
                                };
                                changed = true;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    changed
}

#[allow(
    clippy::unused_async,
    reason = "WASM prepares images inline without a blocking pool"
)]
pub async fn prepare_user_input(input: &PromptInput) -> Vec<ContentItem> {
    let input = match input {
        PromptInput::Text(text) => vec![UserInput::Text { text: text.clone() }],
        PromptInput::Content(items) => items.clone(),
    };
    #[cfg(target_family = "wasm")]
    {
        prepare_user_content(input)
    }
    #[cfg(not(target_family = "wasm"))]
    if !input
        .iter()
        .any(|item| matches!(item, UserInput::Image { .. } | UserInput::LocalImage { .. }))
    {
        return prepare_user_content(input);
    }
    #[cfg(not(target_family = "wasm"))]
    match tokio::task::spawn_blocking(move || prepare_user_content(input)).await {
        Ok(content) => content,
        Err(error) => {
            tracing::warn!(%error, "failed to join user image preparation task");
            vec![input_text(IMAGE_PROCESSING_ERROR_PLACEHOLDER)]
        }
    }
}

pub fn prepare_embedded_output_images(output: &mut ToolOutputBody) {
    if let ToolOutputBody::Content(content) = output {
        *content = prepare_content(std::mem::take(content));
    }
    output.replace_invalid_image_envelopes();
}

pub fn prepare_embedded_user_input(input: &PromptInput) -> Vec<ContentItem> {
    let input = match input {
        PromptInput::Text(text) => vec![UserInput::Text { text: text.clone() }],
        PromptInput::Content(items) => items.clone(),
    };
    prepare_user_content_for_host(input, true)
}

fn prepare_user_content(input: Vec<UserInput>) -> Vec<ContentItem> {
    prepare_user_content_for_host(input, cfg!(target_family = "wasm"))
}

fn prepare_user_content_for_host(input: Vec<UserInput>, embedded: bool) -> Vec<ContentItem> {
    let mut content = Vec::with_capacity(input.len());
    #[cfg(not(target_family = "wasm"))]
    let mut image_index = 0;
    for item in input {
        match item {
            UserInput::Text { text } => content.push(input_text(text)),
            UserInput::ImageFile { file_id, detail } => {
                if nanocode_types_core::responses::valid_image_file_id(&file_id) {
                    content.push(ContentItem::InputImageFile {
                        file_id: file_id.into_boxed_str(),
                        detail,
                    });
                } else {
                    content.push(input_text("Image file reference is malformed"));
                }
            }
            UserInput::Image { image_url, detail } => {
                #[cfg(not(target_family = "wasm"))]
                {
                    image_index += 1;
                }
                content.push(prepare_user_image(
                    image_url,
                    detail.unwrap_or(ImageDetail::High),
                ));
            }
            #[cfg(target_family = "wasm")]
            UserInput::LocalImage { path, .. } => {
                content.push(input_text(format!(
                    "Local image paths are unavailable in browser WASM: {}",
                    path.display()
                )));
            }
            #[cfg(not(target_family = "wasm"))]
            UserInput::LocalImage { path, detail } => {
                if embedded {
                    content.push(input_text(format!(
                        "Local image paths are unavailable in browser WASM: {}",
                        path.display()
                    )));
                    continue;
                }
                image_index += 1;
                let detail = detail.unwrap_or(ImageDetail::High);
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        content.push(input_text(format!(
                            "<image name=[Image #{image_index}] path=\"{}\">",
                            path.display()
                        )));
                        content.push(prepare_user_image(
                            encode_data_url("application/octet-stream", &bytes),
                            detail,
                        ));
                        content.push(input_text("</image>"));
                    }
                    Err(error) => content.push(input_text(format!(
                        "Codex could not read the local image at `{}`: {error}",
                        path.display()
                    ))),
                }
            }
            UserInput::Audio { audio_url } => {
                if embedded {
                    content.push(ContentItem::InputAudio {
                        audio_url: audio_url.into_boxed_str(),
                    });
                } else {
                    content.push(input_text("Codex does not support audio input yet."));
                }
            }
            UserInput::LocalAudio { path } => {
                if embedded {
                    content.push(input_text(format!(
                        "Local audio paths are unavailable in browser WASM: {}",
                        path.display()
                    )));
                } else {
                    content.push(input_text("Codex does not support local audio input yet."));
                }
            }
            UserInput::File { filename, .. } => {
                content.push(input_text(format!(
                    "Codex does not support inline document input yet; document {} was not sent to the model.",
                    filename.as_deref().unwrap_or("attachment")
                )));
            }
        }
    }
    content
}

fn prepare_user_image(mut image_url: String, detail: ImageDetail) -> ContentItem {
    match prepare_image(&mut image_url, detail) {
        Ok(()) => ContentItem::InputImage {
            image_url: image_url.into_boxed_str(),
            detail: Some(detail),
        },
        Err(error) => {
            tracing::warn!(%error, "failed to prepare message image");
            input_text(error.placeholder())
        }
    }
}

fn input_text(text: impl Into<String>) -> ContentItem {
    ContentItem::InputText {
        text: text.into().into_boxed_str(),
    }
}

fn prepare_content(mut content: Vec<ToolOutputContent>) -> Vec<ToolOutputContent> {
    for item in &mut content {
        let ToolOutputContent::InputImage { image_url, detail } = item else {
            continue;
        };
        if let Err(error) = prepare_image(image_url, *detail) {
            tracing::warn!(%error, "failed to prepare tool output image");
            *item = ToolOutputContent::InputText {
                text: error.placeholder().to_owned(),
            };
        }
    }
    content
}

fn prepare_image(image_url: &mut String, detail: ImageDetail) -> Result<(), ImagePreparationError> {
    if is_remote_image_url(image_url) {
        return Err(ImagePreparationError::RemoteUrlUnsupported);
    }
    if !is_data_url(image_url) {
        return Ok(());
    }
    let limits = match detail {
        ImageDetail::Auto | ImageDetail::High => HIGH_DETAIL_LIMITS,
        ImageDetail::Original => ORIGINAL_DETAIL_LIMITS,
        ImageDetail::Low => return Err(ImagePreparationError::UnsupportedLowDetail),
    };
    let bytes = decode_data_url(image_url, MAX_PROMPT_IMAGE_INPUT_BYTES)?;
    *image_url =
        load_for_prompt_bytes(Path::new("<data-url-image>"), bytes, limits)?.into_data_url();
    Ok(())
}

#[allow(
    clippy::unused_async,
    reason = "WASM prepares images inline without a blocking pool"
)]
pub async fn prepare_base64_images(
    images: Vec<String>,
    max_dimension: u32,
) -> Vec<Result<(String, &'static str), &'static str>> {
    if images.is_empty() {
        return Vec::new();
    }
    let prepare = move |images: Vec<String>| {
        images
            .iter()
            .map(|data| prepare_base64_image(data, max_dimension))
            .collect()
    };
    #[cfg(target_family = "wasm")]
    {
        prepare(images)
    }
    #[cfg(not(target_family = "wasm"))]
    {
        let count = images.len();
        match tokio::task::spawn_blocking(move || prepare(images)).await {
            Ok(prepared) => prepared,
            Err(error) => {
                tracing::warn!(%error, "failed to join base64 image preparation task");
                vec![Err(IMAGE_PROCESSING_ERROR_PLACEHOLDER); count]
            }
        }
    }
}

fn prepare_base64_image(
    data: &str,
    max_dimension: u32,
) -> Result<(String, &'static str), &'static str> {
    let limits = PromptImageResizeLimits {
        max_dimension: ORIGINAL_DETAIL_LIMITS.max_dimension.min(max_dimension),
        ..ORIGINAL_DETAIL_LIMITS
    };
    decode_base64(data, MAX_PROMPT_IMAGE_INPUT_BYTES)
        .and_then(|bytes| load_for_prompt_bytes(Path::new("<base64-image>"), bytes, limits))
        .map(|image| {
            (
                base64_simd::STANDARD.encode_to_string(image.bytes),
                image.mime,
            )
        })
        .map_err(|error| {
            tracing::warn!(%error, "failed to prepare base64 image");
            error.placeholder()
        })
}

fn is_remote_image_url(image_url: &str) -> bool {
    image_url.split_once(':').is_some_and(|(scheme, _)| {
        scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
    })
}

fn is_data_url(image_url: &str) -> bool {
    image_url
        .get(..DATA_URL_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(DATA_URL_PREFIX))
}

fn decode_data_url(
    image_url: &str,
    max_input_bytes: usize,
) -> Result<Vec<u8>, ImagePreparationError> {
    let rest = image_url
        .get(..DATA_URL_PREFIX.len())
        .filter(|prefix| prefix.eq_ignore_ascii_case(DATA_URL_PREFIX))
        .and_then(|_| image_url.get(DATA_URL_PREFIX.len()..))
        .ok_or_else(|| ImagePreparationError::Processing("missing data: prefix".to_owned()))?;
    let (metadata, encoded) = rest.split_once(',').ok_or_else(|| {
        ImagePreparationError::Processing("data URL is missing a comma separator".to_owned())
    })?;
    if !metadata
        .split(';')
        .any(|part| part.eq_ignore_ascii_case("base64"))
    {
        return Err(ImagePreparationError::Processing(
            "only base64 data URLs are supported".to_owned(),
        ));
    }
    decode_base64(encoded, max_input_bytes)
}

fn decode_base64(encoded: &str, max_input_bytes: usize) -> Result<Vec<u8>, ImagePreparationError> {
    if encoded.len() > max_input_bytes {
        return Err(ImagePreparationError::ImageTooLarge {
            representation: "base64 payload",
            size: encoded.len(),
            max: max_input_bytes,
        });
    }
    let bytes = base64_simd::STANDARD
        .decode_to_vec(encoded)
        .or_else(|_| BASE64_STANDARD.decode(encoded))
        .map_err(|error| {
            ImagePreparationError::Processing(format!("invalid base64 payload: {error}"))
        })?;
    if bytes.len() > max_input_bytes {
        return Err(ImagePreparationError::ImageTooLarge {
            representation: "decoded input",
            size: bytes.len(),
            max: max_input_bytes,
        });
    }
    Ok(bytes)
}

fn load_for_prompt_bytes(
    path: &Path,
    mut file_bytes: Vec<u8>,
    limits: PromptImageResizeLimits,
) -> Result<EncodedImage, ImagePreparationError> {
    let key = ImageCacheKey {
        digest: Sha1::digest(&file_bytes).into(),
        limits,
    };
    if let Some(image) = cached_image(&key) {
        return Ok(image);
    }

    let guessed_format = image::guess_format(&file_bytes).map_err(|error| {
        ImagePreparationError::Processing(format!(
            "unable to identify image at `{}`: {error}",
            path.display()
        ))
    })?;
    let gif_icc_profile = if guessed_format == ImageFormat::Gif {
        let profile = metadata::gif_icc(&file_bytes);
        metadata::normalize_gif_header(&mut file_bytes)?;
        profile
    } else {
        None
    };
    let preserved_format = match guessed_format {
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP => Some(guessed_format),
        _ => None,
    };
    let (dynamic, metadata) = if let Some(dynamic) = pam::decode_alpha(&file_bytes)? {
        (
            dynamic,
            ImageMetadata {
                icc_profile: None,
                exif: None,
            },
        )
    } else {
        let mut reader =
            ImageReader::with_format(Cursor::new(file_bytes.as_slice()), guessed_format);
        let mut decode_limits = image::Limits::default();
        decode_limits.max_alloc = Some(MAX_PROMPT_IMAGE_DECODE_BYTES);
        reader.limits(decode_limits);
        let mut decoder = reader.into_decoder().map_err(|error| {
            ImagePreparationError::Processing(format!(
                "unable to decode image at `{}`: {error}",
                path.display()
            ))
        })?;
        let pixel_bytes = decoder.total_bytes();
        if pixel_bytes > MAX_PROMPT_IMAGE_DECODE_BYTES {
            return Err(ImagePreparationError::ImageTooLarge {
                representation: "decoded pixels",
                size: usize::try_from(pixel_bytes).unwrap_or(usize::MAX),
                max: usize::try_from(MAX_PROMPT_IMAGE_DECODE_BYTES).unwrap_or(usize::MAX),
            });
        }
        let metadata = ImageMetadata {
            icc_profile: decoder
                .icc_profile()
                .ok()
                .flatten()
                .or(gif_icc_profile)
                .filter(|profile| profile.get(16..20) == Some(b"RGB ")),
            exif: decoder.exif_metadata().ok().flatten().or_else(|| {
                (guessed_format == ImageFormat::Png)
                    .then(|| metadata::png_exif(&file_bytes))
                    .flatten()
            }),
        };
        let dynamic = DynamicImage::from_decoder(decoder).map_err(|error| {
            ImagePreparationError::Processing(format!(
                "unable to decode image at `{}`: {error}",
                path.display()
            ))
        })?;
        (dynamic, metadata)
    };
    let (width, height) = dynamic.dimensions();
    let (target_width, target_height) =
        prompt_image_output_dimensions_for_limits(width, height, limits);

    let image = if (target_width, target_height) == (width, height) {
        if let Some(format) = preserved_format {
            EncodedImage {
                bytes: file_bytes.into(),
                mime: format_to_mime(format),
            }
        } else {
            encode_image(&dynamic, ImageFormat::Png, metadata)?
        }
    } else {
        let resized = dynamic.thumbnail_exact(target_width, target_height);
        drop(dynamic);
        encode_image(
            &resized,
            preserved_format.unwrap_or(ImageFormat::Png),
            metadata,
        )?
    };

    cache_image(key, image.clone());
    Ok(image)
}

#[cfg(not(target_family = "wasm"))]
pub fn load_for_prompt_data_url(
    path: &Path,
    file_bytes: Vec<u8>,
    detail: ImageDetail,
) -> Result<String, String> {
    let limits = match detail {
        ImageDetail::Auto | ImageDetail::High => HIGH_DETAIL_LIMITS,
        ImageDetail::Original => ORIGINAL_DETAIL_LIMITS,
        ImageDetail::Low => return Err("image detail `low` is not supported".to_owned()),
    };
    load_for_prompt_bytes(path, file_bytes, limits)
        .map(EncodedImage::into_data_url)
        .map_err(|error| error.to_string())
}

fn prompt_image_output_dimensions_for_limits(
    width: u32,
    height: u32,
    limits: PromptImageResizeLimits,
) -> (u32, u32) {
    let width = width.max(1);
    let height = height.max(1);
    if prompt_image_dimensions_fit(width, height, limits) {
        return (width, height);
    }

    let max_dimension_scale =
        (f64::from(limits.max_dimension) / f64::from(width.max(height))).min(1.0);
    let width = rounded_dimension(f64::from(width) * max_dimension_scale);
    let height = rounded_dimension(f64::from(height) * max_dimension_scale);
    if prompt_image_dimensions_fit(width, height, limits) {
        return (width, height);
    }

    let width_f64 = f64::from(width);
    let height_f64 = f64::from(height);
    let patch_size = f64::from(PROMPT_IMAGE_PATCH_SIZE);
    let mut scale =
        (patch_size * patch_size * f64::from(limits.max_patches) / width_f64 / height_f64).sqrt();
    let scaled_patches_wide = width_f64 * scale / patch_size;
    let scaled_patches_high = height_f64 * scale / patch_size;
    scale *= (scaled_patches_wide.floor() / scaled_patches_wide)
        .min(scaled_patches_high.floor() / scaled_patches_high);

    (
        floored_dimension(width_f64 * scale),
        floored_dimension(height_f64 * scale),
    )
}

fn prompt_image_dimensions_fit(width: u32, height: u32, limits: PromptImageResizeLimits) -> bool {
    let patches_wide = width.div_ceil(PROMPT_IMAGE_PATCH_SIZE);
    let patches_high = height.div_ceil(PROMPT_IMAGE_PATCH_SIZE);
    let patch_count = u64::from(patches_wide) * u64::from(patches_high);
    width <= limits.max_dimension
        && height <= limits.max_dimension
        && patch_count <= u64::from(limits.max_patches)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn rounded_dimension(value: f64) -> u32 {
    (value.round() as u32).max(1)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn floored_dimension(value: f64) -> u32 {
    (value.floor() as u32).max(1)
}

fn encode_image(
    image: &DynamicImage,
    preferred_format: ImageFormat,
    metadata: ImageMetadata,
) -> Result<EncodedImage, ImagePreparationError> {
    let target_format = match preferred_format {
        ImageFormat::Jpeg => ImageFormat::Jpeg,
        ImageFormat::WebP => ImageFormat::WebP,
        _ => ImageFormat::Png,
    };
    let mut bytes = Vec::new();
    let ImageMetadata { icc_profile, exif } = metadata;
    match target_format {
        ImageFormat::Png => {
            let (pixels, color) = rgb_or_rgba8_bytes(image);
            metadata::encode_png(
                &mut bytes,
                pixels.as_ref(),
                image.width(),
                image.height(),
                color,
                icc_profile,
                exif,
            )?;
        }
        ImageFormat::Jpeg => {
            let mut encoder = JpegEncoder::new_with_quality(&mut bytes, 85);
            if let Some(profile) = icc_profile {
                encoder.set_icc_profile(profile).map_err(|error| {
                    encode_error(target_format, &image::ImageError::Unsupported(error))
                })?;
            }
            encoder
                .encode_image(image)
                .map_err(|error| encode_error(target_format, &error))?;
            if let Some(exif) = exif {
                metadata::insert_jpeg_exif(&mut bytes, &exif)?;
            }
        }
        ImageFormat::WebP => {
            let (pixels, color) = rgb_or_rgba8_bytes(image);
            let mut encoder = image_webp::WebPEncoder::new(&mut bytes);
            if let Some(profile) = icc_profile {
                encoder.set_icc_profile(profile);
            }
            if let Some(exif) = exif {
                encoder.set_exif_metadata(exif);
            }
            let color = match color {
                ColorType::Rgb8 => image_webp::ColorType::Rgb8,
                ColorType::Rgba8 => image_webp::ColorType::Rgba8,
                _ => unreachable!("pixels are converted to RGB or RGBA"),
            };
            encoder
                .encode(pixels.as_ref(), image.width(), image.height(), color)
                .map_err(|error| {
                    ImagePreparationError::Processing(format!(
                        "unable to encode image as WebP: {error}"
                    ))
                })?;
        }
        _ => unreachable!("target format is normalized above"),
    }
    Ok(EncodedImage {
        bytes: bytes.into(),
        mime: format_to_mime(target_format),
    })
}

fn rgb_or_rgba8_bytes(image: &DynamicImage) -> (Cow<'_, [u8]>, ColorType) {
    match image.color() {
        color @ (ColorType::Rgb8 | ColorType::Rgba8) => (Cow::Borrowed(image.as_bytes()), color),
        _ => (Cow::Owned(image.to_rgba8().into_raw()), ColorType::Rgba8),
    }
}

fn encode_error(format: ImageFormat, error: &image::ImageError) -> ImagePreparationError {
    ImagePreparationError::Processing(format!("unable to encode image as {format:?}: {error}"))
}

const fn format_to_mime(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::WebP => "image/webp",
        _ => "image/png",
    }
}
