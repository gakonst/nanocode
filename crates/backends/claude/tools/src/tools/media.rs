use crate::tools::{ImageSource, SessionEnvironment, ToolOutput, ToolResultBlock};
use nanocode_sys::base64::STANDARD;
#[cfg(unix)]
use nanocode_sys::libc;
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const MAX_SOURCE: usize = 16 * 1024 * 1024;
const MAX_IMAGE: usize = 5 * 1024 * 1024;
const MAX_MEDIA: usize = 20 * 1024 * 1024;
const MAX_PDF: u64 = 20 * 1024 * 1024;
const PDF_PAGES_AT_ONCE: u32 = 10;
const PDF_PAGES_PER_READ: u32 = 20;
const PDFINFO_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_TEXT: usize = 64 * 1024;
const MAX_BLOCKS: usize = 256;

#[derive(Clone, Debug)]
pub struct MediaReadOptions {
    pub pdfinfo: PathBuf,

    pub pdftoppm: PathBuf,
}
impl Default for MediaReadOptions {
    fn default() -> Self {
        Self {
            pdfinfo: "pdfinfo".into(),
            pdftoppm: "pdftoppm".into(),
        }
    }
}

pub(crate) fn read(
    path: &Path,
    input: &Value,
    options: &MediaReadOptions,
    session: Option<&SessionEnvironment>,
) -> Result<Option<ToolOutput>, String> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let mut prefix = [0; 16];
    let count = File::open(path)
        .map_err(|e| format!("read: {e}"))?
        .read(&mut prefix)
        .map_err(|e| format!("read: {e}"))?;
    let signature = nanocode_image::guess_format(&prefix[..count]);
    let pdf = prefix[..count].starts_with(b"%PDF-") || extension == "pdf";
    let is_image = signature.is_some()
        || matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp");
    if !pdf && !is_image && extension != "ipynb" {
        return Ok(None);
    }
    if !pdf && input.get("pages").is_some() {
        return Err("pages is only applicable to PDF files".into());
    }
    if (pdf || is_image) && (input.get("offset").is_some() || input.get("limit").is_some()) {
        return Err(
            "offset and limit apply to text lines or notebook cells; use pages for PDF files"
                .into(),
        );
    }
    if pdf && input.get("pages").is_none() {
        return read_whole_pdf(path, options, session).map(Some);
    }
    let bytes = read_bytes(path, MAX_SOURCE)?;
    if pdf {
        return read_pdf(&bytes, input, options, session).map(Some);
    }
    if is_image {
        let block = image_block(&bytes)?;
        return Ok(Some(
            ToolOutput::content(vec![block])
                .with_metadata(json!({"kind":"image","bytes":bytes.len()})),
        ));
    }
    read_notebook(&bytes, input).map(Some)
}

fn read_bytes(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| format!("read media: {e}"))?
        .take((max + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read media: {e}"))?;
    if bytes.len() > max {
        return Err(format!("media exceeds {} MiB limit", max / (1024 * 1024)));
    }
    Ok(bytes)
}

fn image_block(bytes: &[u8]) -> Result<ToolResultBlock, String> {
    if bytes.len() > MAX_SOURCE {
        return Err("image source exceeds 16 MiB limit".into());
    }
    let format = nanocode_image::guess_format(bytes).ok_or_else(|| {
        format!(
            "invalid image: {}",
            nanocode_image::DecodeError::UnknownFormat
        )
    })?;
    let source = super::image::media_type(format)?;
    let mut mime = source.unwrap_or("image/png");
    let decoded = super::image::decode(bytes, format)?;

    let pixels = f64::from(decoded.width()) * f64::from(decoded.height());
    let scale = (1568.0 / f64::from(decoded.width().max(decoded.height())))
        .min((1_150_000.0 / pixels).sqrt())
        .min(1.0);
    let resized = if scale < 1.0 {
        nanocode_image::lanczos3(
            &decoded,
            (f64::from(decoded.width()) * scale).floor().max(1.0) as u32,
            (f64::from(decoded.height()) * scale).floor().max(1.0) as u32,
        )
    } else {
        decoded
    };
    let mut encoded = bytes.to_vec();
    if scale < 1.0 || source.is_none() {
        encoded.clear();
        encoded = nanocode_image::encode_png(&resized);
        mime = "image/png";
    }
    if encoded.len() > 500 * 1024 {
        let rgb: Vec<u8> = resized
            .to_rgba8()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| {
                let alpha = u16::from(p[3]);
                [0, 1, 2]
                    .map(|i| ((u16::from(p[i]) * alpha + 255 * (255 - alpha) + 127) / 255) as u8)
            })
            .collect();
        for quality in [85, 70, 55, 40, 25, 10] {
            encoded = nanocode_jpeg::encode(
                &rgb,
                resized.width(),
                resized.height(),
                nanocode_jpeg::Color::Rgb,
                quality,
                None,
            )
            .map_err(|e| format!("image compression: {e}"))?;
            if encoded.len() <= 500 * 1024 {
                break;
            }
        }
        mime = "image/jpeg";
    }
    if encoded.len() > MAX_IMAGE {
        return Err("normalized image exceeds 5 MiB".into());
    }
    nanocode_image::remember(&encoded, resized.width(), resized.height());
    Ok(ToolResultBlock::Image {
        source: ImageSource::Base64 {
            media_type: mime.into(),
            data: STANDARD.encode(&encoded),
        },
    })
}

fn page_range(input: &Value, total: u32) -> Result<(u32, u32), String> {
    let Some(value) = input.get("pages") else {
        if total > 10 {
            return Err(format!(
                "PDF has {total} pages; specify pages (maximum 20 per request)"
            ));
        }
        return Ok((1, total));
    };
    let value = value
        .as_str()
        .ok_or("invalid pages; use a page or inclusive range such as 1-5")?;
    if value.len() > 32 {
        return Err("invalid pages".into());
    }
    let mut parts = value.split('-');
    let start = parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .ok_or("invalid pages")?;
    let end = match parts.next() {
        Some(s) => s.parse::<u32>().map_err(|_| "invalid pages")?,
        None => start,
    };
    if parts.next().is_some() || start == 0 || end < start || end > total {
        return Err(format!("pages must be within 1-{total}"));
    }
    if end - start >= 20 {
        return Err("PDF reads are limited to 20 pages per request".into());
    }
    Ok((start, end))
}

fn read_whole_pdf(
    path: &Path,
    options: &MediaReadOptions,
    session: Option<&SessionEnvironment>,
) -> Result<ToolOutput, String> {
    let mut info = Command::new(&options.pdfinfo);
    info.arg(path).env("LC_ALL", "C");
    SessionEnvironment::apply_or_clear(session, &mut info);
    let pages = helper(&mut info, 16 * 1024, Instant::now() + PDFINFO_TIMEOUT)
        .ok()
        .and_then(|info| {
            String::from_utf8(info).ok()?.lines().find_map(|l| {
                l.strip_prefix("Pages:")
                    .and_then(|n| n.trim().parse::<u32>().ok())
            })
        });
    if let Some(pages) = pages.filter(|&n| n > PDF_PAGES_AT_ONCE) {
        return Err(format!(
            "This PDF has {pages} pages, which is too many to read at once. Use the pages parameter to read specific page ranges (e.g., pages: \"1-5\"). Maximum {PDF_PAGES_PER_READ} pages per request."
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| format!("read: {e}"))?
        .take(MAX_PDF + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read: {e}"))?;
    if bytes.is_empty() {
        return Err(format!("PDF file is empty: {}", path.display()));
    }
    if bytes.len() as u64 > MAX_PDF {
        return Err(format!(
            "PDF file exceeds maximum allowed size of {}.",
            super::text_files::size_text(MAX_PDF)
        ));
    }
    if !bytes.starts_with(b"%PDF-") {
        return Err(format!(
            "File is not a valid PDF (missing %PDF- header): {}",
            path.display()
        ));
    }
    Ok(ToolOutput::content(vec![
        ToolResultBlock::Text {
            text: format!(
                "PDF file read: {} ({})",
                path.display(),
                super::text_files::size_text(bytes.len() as u64)
            ),
        },
        ToolResultBlock::Document {
            file_data: format!("data:application/pdf;base64,{}", STANDARD.encode(&bytes)),
        },
    ])
    .with_metadata(json!({"kind":"pdf","bytes":bytes.len()})))
}

fn read_pdf(
    bytes: &[u8],
    input: &Value,
    options: &MediaReadOptions,
    session: Option<&SessionEnvironment>,
) -> Result<ToolOutput, String> {
    if !bytes.starts_with(b"%PDF-") {
        return Err("invalid PDF header".into());
    }

    let mut snapshot =
        nanocode_platform::TemporaryFile::new().map_err(|e| format!("PDF snapshot: {e}"))?;
    snapshot
        .write_all(bytes)
        .map_err(|e| format!("PDF snapshot: {e}"))?;
    snapshot.flush().map_err(|e| format!("PDF snapshot: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut info = Command::new(&options.pdfinfo);
    info.arg(snapshot.path()).env("LC_ALL", "C");
    SessionEnvironment::apply_or_clear(session, &mut info);
    let info = helper(&mut info, 16 * 1024, deadline)?;
    let info = String::from_utf8(info).map_err(|_| "pdfinfo returned invalid text")?;
    if info
        .lines()
        .any(|l| l.starts_with("Encrypted:") && l.split_whitespace().nth(1) != Some("no"))
    {
        return Err("encrypted PDFs are not supported".into());
    }
    let pages = info
        .lines()
        .find_map(|l| {
            l.strip_prefix("Pages:")
                .and_then(|n| n.trim().parse::<u32>().ok())
        })
        .filter(|&n| n > 0)
        .ok_or("PDF has no readable pages")?;
    let (first, last) = page_range(input, pages)?;
    let mut blocks = Vec::new();
    let mut size = 0;
    for page in first..=last {
        let mut render = Command::new(&options.pdftoppm);
        render
            .args([
                "-f",
                &page.to_string(),
                "-l",
                &page.to_string(),
                "-singlefile",
                "-scale-to",
                "1600",
                "-png",
            ])
            .arg(snapshot.path())
            .env("LC_ALL", "C");
        SessionEnvironment::apply_or_clear(session, &mut render);
        let raster = helper(&mut render, MAX_IMAGE, deadline)?;
        size += raster.len();
        if size > MAX_MEDIA {
            return Err("PDF rendered output exceeds 20 MiB; request fewer pages".into());
        }
        blocks.push(ToolResultBlock::Text {
            text: format!("PDF page {page} of {pages}\n"),
        });
        blocks.push(image_block(&raster)?);
    }
    Ok(ToolOutput::content(blocks).with_metadata(
        json!({"kind":"pdf","total_pages":pages,"first_page":first,"last_page":last}),
    ))
}

fn helper(command: &mut Command, max: usize, deadline: Instant) -> Result<Vec<u8>, String> {
    if Instant::now() >= deadline {
        return Err("PDF rendering exceeded 30 seconds".into());
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("PDF helper unavailable: {e}; install Poppler pdfinfo and pdftoppm or configure MediaReadOptions"))?;
    let stdout = child.stdout.take().ok_or("PDF helper stdout unavailable")?;
    let stderr = child.stderr.take().ok_or("PDF helper stderr unavailable")?;
    let (send, recv) = std::sync::mpsc::channel();
    let send_err = send.clone();
    let out_thread = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take((max + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("PDF helper output: {e}"))
            .and_then(|_| {
                if bytes.len() > max {
                    Err("PDF helper output exceeds limit".into())
                } else {
                    Ok(bytes)
                }
            });
        let _ = send.send((true, result));
    });
    let err_thread = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stderr
            .take(16 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("PDF helper diagnostic: {e}"))
            .and_then(|_| {
                if bytes.len() > 16 * 1024 {
                    Err("PDF helper diagnostics exceed limit".into())
                } else {
                    Ok(bytes)
                }
            });
        let _ = send_err.send((false, result));
    });
    let result = (|| {
        let mut out = None;
        let mut err = None;
        while out.is_none() || err.is_none() {
            let wait = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(10));
            if wait.is_zero() {
                return Err("PDF rendering exceeded 30 seconds".into());
            }
            match recv.recv_timeout(wait) {
                Ok((true, bytes)) => out = Some(bytes?),
                Ok((false, bytes)) => err = Some(bytes?),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err("PDF helper output disconnected".into()),
            }
        }
        loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|e| format!("PDF helper wait: {e}"))?
            {
                if !status.success() {
                    return Err(format!(
                        "PDF helper failed: {}",
                        String::from_utf8_lossy(&err.unwrap_or_default()).trim()
                    ));
                }
                return Ok(out.unwrap_or_default());
            }
            if Instant::now() >= deadline {
                return Err("PDF rendering exceeded 30 seconds".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    #[cfg(unix)]
    {
        let _ = nanocode_platform::unix::signal_group(child.id() as i32, libc::SIGKILL);
    }
    if result.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();

    if out_thread.is_finished() {
        let _ = out_thread.join();
    }
    if err_thread.is_finished() {
        let _ = err_thread.join();
    }
    result
}

fn notebook_text(value: &Value) -> Result<String, String> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Array(parts) => parts
            .iter()
            .map(|p| {
                p.as_str()
                    .ok_or_else(|| "invalid notebook text array".to_string())
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|p| p.concat()),
        _ => Err("invalid notebook text".into()),
    }
}

fn push_text(
    blocks: &mut Vec<ToolResultBlock>,
    used: &mut usize,
    text: String,
) -> Result<(), String> {
    *used += text.len();
    if *used > MAX_TEXT || blocks.len() >= MAX_BLOCKS {
        return Err(
            "notebook output exceeds limit; use offset and limit to select fewer cells".into(),
        );
    }
    blocks.push(ToolResultBlock::Text { text });
    Ok(())
}

fn read_notebook(bytes: &[u8], input: &Value) -> Result<ToolOutput, String> {
    let notebook: Value =
        serde_json::from_slice(bytes).map_err(|e| format!("invalid notebook JSON: {e}"))?;
    if notebook.get("nbformat").and_then(Value::as_u64) != Some(4) {
        return Err("Read supports nbformat 4 notebooks".into());
    }
    let cells = notebook
        .get("cells")
        .and_then(Value::as_array)
        .ok_or("notebook has no cells array")?;
    let offset = input
        .get("offset")
        .map_or(Some(1), Value::as_u64)
        .filter(|&n| n > 0)
        .ok_or("invalid offset")?;
    let limit = input
        .get("limit")
        .map_or(Some(2000), Value::as_u64)
        .filter(|&n| n > 0)
        .ok_or("invalid limit")?;
    let mut blocks = Vec::new();
    let mut text_bytes = 0;
    let mut media_bytes = 0;
    for (index, cell) in cells
        .iter()
        .enumerate()
        .skip(offset.saturating_sub(1).min(usize::MAX as u64) as usize)
        .take(limit.min(2000) as usize)
    {
        let kind = cell
            .get("cell_type")
            .and_then(Value::as_str)
            .ok_or("notebook cell has no cell_type")?;
        let id = cell
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| index.to_string());
        let source = notebook_text(cell.get("source").ok_or("notebook cell has no source")?)?;
        push_text(
            &mut blocks,
            &mut text_bytes,
            format!("Cell {id} ({kind}, index {index})\n{source}\n"),
        )?;
        if let Some(outputs) = cell.get("outputs") {
            for output in outputs
                .as_array()
                .ok_or("notebook outputs must be an array")?
            {
                if let Some(text) = output.get("text") {
                    push_text(&mut blocks, &mut text_bytes, notebook_text(text)?)?;
                }
                if output.get("output_type").and_then(Value::as_str) == Some("error") {
                    push_text(
                        &mut blocks,
                        &mut text_bytes,
                        format!(
                            "{}: {}\n{}\n",
                            output
                                .get("ename")
                                .and_then(Value::as_str)
                                .unwrap_or("Error"),
                            output.get("evalue").and_then(Value::as_str).unwrap_or(""),
                            output
                                .get("traceback")
                                .map(notebook_text)
                                .transpose()?
                                .unwrap_or_default()
                        ),
                    )?;
                }
                if let Some(data) = output.get("data").and_then(Value::as_object) {
                    let mut image = false;
                    for mime in ["image/png", "image/jpeg", "image/gif", "image/webp"] {
                        if let Some(value) = data.get(mime) {
                            let encoded = notebook_text(value)?;
                            if encoded.len() > MAX_IMAGE * 4 / 3 + 1024 {
                                return Err("notebook image exceeds 5 MiB limit".into());
                            }
                            let encoded: String = encoded
                                .chars()
                                .filter(|c| !c.is_ascii_whitespace())
                                .collect();
                            let bytes = STANDARD
                                .decode(encoded)
                                .map_err(|e| format!("invalid notebook image base64: {e}"))?;
                            media_bytes += bytes.len();
                            if media_bytes > MAX_MEDIA || blocks.len() >= MAX_BLOCKS {
                                return Err(
                                    "notebook media output exceeds limit; select fewer cells"
                                        .into(),
                                );
                            }

                            let actual = match nanocode_image::guess_format(&bytes) {
                                Some(nanocode_image::Format::Png) => "image/png",
                                Some(nanocode_image::Format::Jpeg) => "image/jpeg",
                                Some(nanocode_image::Format::Gif) => "image/gif",
                                Some(nanocode_image::Format::WebP) => "image/webp",
                                _ => "unsupported",
                            };
                            if actual != mime {
                                return Err(
                                    "notebook image MIME type does not match its bytes".into()
                                );
                            }
                            let block = image_block(&bytes)?;
                            blocks.push(block);
                            image = true;
                            break;
                        }
                    }
                    if let Some(text) = data.get("text/plain") {
                        push_text(&mut blocks, &mut text_bytes, notebook_text(text)?)?;
                    } else if !image {
                        for (mime, value) in data {
                            let text = if mime == "application/json" {
                                value.to_string()
                            } else if value.is_string() || value.is_array() {
                                notebook_text(value)?
                            } else {
                                value.to_string()
                            };
                            push_text(
                                &mut blocks,
                                &mut text_bytes,
                                format!("Output ({mime})\n{text}\n"),
                            )?;
                        }
                    }
                }
            }
        }
    }
    if blocks.is_empty() {
        blocks.push(ToolResultBlock::Text {
            text: "No notebook cells in the requested range.\n".into(),
        });
    }
    Ok(ToolOutput::content(blocks)
        .with_metadata(json!({"kind":"notebook","total_cells":cells.len(),"offset":offset})))
}
