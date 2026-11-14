use crate::fetch::{Failure, Network, Request, Response, current};
use nanocode_bytes::Bytes;
use psh_engine::{
    host::{OpenOptions, error_text},
    util::{LocalBoxFuture, Mode, UtilCtx, UtilEntry},
};
use std::time::Duration;

pub(crate) const UTILITIES: [UtilEntry; 2] = [
    UtilEntry {
        names: &["curl"],
        mode: Mode::Always,
        destructive: false,
        run: curl_entry,
    },
    UtilEntry {
        names: &["wget"],
        mode: Mode::Always,
        destructive: false,
        run: wget_entry,
    },
];

const UNAVAILABLE: &str = "network access is not available in this shell";
const AGENT: &str = "curl/8.7.1";
const WGET_AGENT: &str = "Wget/1.25.0";
const WGET_REDIRECTS: u32 = 20;
const CURL_REDIRECTS: u32 = 50;

fn curl_entry<'a>(ctx: &'a mut UtilCtx<'_>, argv: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
    Box::pin(curl(ctx, argv))
}

fn wget_entry<'a>(ctx: &'a mut UtilCtx<'_>, argv: &'a [Vec<u8>]) -> LocalBoxFuture<'a, i32> {
    Box::pin(wget(ctx, argv))
}

struct Parts {
    scheme: String,
    host: String,
    port: u16,
    path: String,
}

fn parts(url: &str) -> Option<Parts> {
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = rest[..end].rsplit('@').next()?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            (host, port.parse().ok()?)
        }
        _ => (authority, if scheme == "https" { 443 } else { 80 }),
    };
    if host.is_empty() {
        return None;
    }
    let path = rest[end..].split(['?', '#']).next().unwrap_or_default();
    Some(Parts {
        scheme: scheme.to_ascii_lowercase(),
        host: host.to_owned(),
        port,
        path: path.to_owned(),
    })
}

fn with_scheme(url: &str) -> String {
    if url.contains("://") {
        url.to_owned()
    } else {
        format!("http://{url}")
    }
}

fn base64(data: &[u8]) -> String {
    const SYMBOLS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let mut wide = [0u8; 3];
        wide[..chunk.len()].copy_from_slice(chunk);
        let bits = u32::from(wide[0]) << 16 | u32::from(wide[1]) << 8 | u32::from(wide[2]);
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(SYMBOLS[(bits >> (18 - 6 * index) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn percent(data: &[u8]) -> String {
    let mut out = String::new();
    for &byte in data {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else if byte == b' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    headers.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
    headers.push((name.to_owned(), value.to_owned()));
}

async fn read_file(ctx: &mut UtilCtx<'_>, path: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut data = Vec::new();
    if path == b"-" {
        ctx.read_to_end(&mut data).await?;
    } else {
        let file = ctx.open(path, OpenOptions::read()).await?;
        file.read_to_end(&mut data).await?;
    }
    Ok(data)
}

async fn write_file(ctx: &mut UtilCtx<'_>, path: &[u8], data: &[u8]) -> std::io::Result<()> {
    let file = ctx.open(path, OpenOptions::write()).await?;
    file.write_all(data).await
}

fn head_text(response: &Response) -> Vec<u8> {
    let mut out = format!("{} {}", response.protocol, response.status);
    out.push(' ');
    out.push_str(&response.reason);
    out.push_str("\r\n");
    for (name, value) in &response.headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("\r\n");
    out.into_bytes()
}

enum Target {
    Stdout,
    File(Vec<u8>),
    Remote,
}

#[derive(Default)]
struct Curl {
    urls: Vec<String>,
    targets: Vec<Target>,
    method: Option<String>,
    headers: Vec<String>,
    data: Vec<Vec<u8>>,
    query: bool,
    json: bool,
    head: bool,
    include: bool,
    silent: bool,
    show_error: bool,
    fail: bool,
    follow: bool,
    redirects: Option<u32>,
    write_out: Option<String>,
    user: Option<String>,
    bearer: Option<String>,
    agent: Option<String>,
    referer: Option<String>,
    cookie: Option<String>,
    range: Option<String>,
    compressed: bool,
    timeout: Option<Duration>,
    dump_header: Option<Vec<u8>>,
    upload: Option<Vec<u8>>,
}

enum Problem {
    Usage(String),
    Lacks(String),
}

fn absolute(base: &str, location: &str) -> String {
    if location.contains("://") {
        return location.to_owned();
    }
    let Some((scheme, rest)) = base.split_once("://") else {
        return location.to_owned();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let origin = format!("{scheme}://{}", &rest[..end]);
    if location.starts_with('/') {
        return format!("{origin}{location}");
    }
    let path = rest[end..].split(['?', '#']).next().unwrap_or_default();
    let directory = path.rfind('/').map_or("/", |cut| &path[..=cut]);
    format!("{origin}{directory}{location}")
}

fn curl_failure(
    failure: &Failure,
    url: &str,
    timeout: Option<Duration>,
    redirects: u32,
) -> (i32, String) {
    let host = parts(url).map(|parts| (parts.host, parts.port));
    let (name, port) = host.unwrap_or_default();
    match failure {
        Failure::Resolve => (6, format!("Could not resolve host: {name}")),
        Failure::Connect => (
            7,
            format!("Failed to connect to {name} port {port}: Couldn't connect to server"),
        ),
        Failure::Timeout => (
            28,
            format!(
                "Operation timed out after {} milliseconds",
                timeout.map_or(0, |limit| limit.as_millis())
            ),
        ),
        Failure::Redirects => (47, format!("Maximum ({redirects}) redirects followed")),
        Failure::Secure(detail) => (60, detail.clone()),
        Failure::TooLarge => (63, "Maximum file size exceeded".to_owned()),
        Failure::Denied(detail) => (7, format!("{detail}: {UNAVAILABLE}")),
        Failure::Other(detail) => (56, detail.clone()),
    }
}

async fn curl_options(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> Result<Curl, Problem> {
    let mut options = Curl::default();
    let mut args = Vec::new();
    for arg in &argv[1..] {
        match std::str::from_utf8(arg) {
            Ok(text) => args.push(text.to_owned()),
            Err(_) => return Err(Problem::Usage("an argument is not UTF-8".to_owned())),
        }
    }
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].clone();
        index += 1;
        if !arg.starts_with('-') || arg == "-" {
            options.urls.push(arg);
            continue;
        }
        let (names, attached): (Vec<String>, Option<String>) =
            if let Some(long) = arg.strip_prefix("--") {
                match long.split_once('=') {
                    Some((name, value)) => (vec![format!("--{name}")], Some(value.to_owned())),
                    None => (vec![arg.clone()], None),
                }
            } else {
                let mut names = Vec::new();
                let mut attached = None;
                let letters: Vec<char> = arg[1..].chars().collect();
                for (at, letter) in letters.iter().enumerate() {
                    names.push(format!("-{letter}"));
                    if "XHdowuAebmFTxcrCD".contains(*letter) {
                        let rest: String = letters[at + 1..].iter().collect();
                        if !rest.is_empty() {
                            attached = Some(rest);
                        }
                        break;
                    }
                }
                (names, attached)
            };
        for name in names {
            let mut value = || -> Result<String, Problem> {
                if let Some(value) = attached.clone() {
                    return Ok(value);
                }
                let value = args
                    .get(index)
                    .cloned()
                    .ok_or_else(|| Problem::Usage(format!("option {name}: requires parameter")))?;
                index += 1;
                Ok(value)
            };
            match name.as_str() {
                "-s" | "--silent" => options.silent = true,
                "-S" | "--show-error" => options.show_error = true,
                "-f" | "--fail" | "--fail-with-body" => options.fail = true,
                "-L" | "--location" => options.follow = true,
                "-I" | "--head" => options.head = true,
                "-i" | "--include" => options.include = true,
                "-G" | "--get" => options.query = true,
                "-O" | "--remote-name" => options.targets.push(Target::Remote),
                "--compressed" => options.compressed = true,
                "-k"
                | "--insecure"
                | "-#"
                | "--progress-bar"
                | "-N"
                | "--no-buffer"
                | "-g"
                | "--globoff"
                | "--no-progress-meter"
                | "--http1.1"
                | "--http2"
                | "-4"
                | "-6"
                | "--tlsv1.2"
                | "--tlsv1.3"
                | "--no-keepalive"
                | "--location-trusted"
                | "--create-dirs"
                | "-J"
                | "--remote-header-name"
                | "-q"
                | "--disable" => {}
                "-o" | "--output" => {
                    let path = value()?;
                    options.targets.push(if path == "-" {
                        Target::Stdout
                    } else {
                        Target::File(path.into_bytes())
                    });
                }
                "-X" | "--request" => options.method = Some(value()?),
                "-H" | "--header" => options.headers.push(value()?),
                "-d" | "--data" | "--data-ascii" | "--data-raw" | "--data-binary" | "--json" => {
                    let text = value()?;
                    let raw = name == "--data-raw";
                    let mut piece = match text.strip_prefix('@') {
                        Some(path) if !raw => {
                            read_file(ctx, path.as_bytes()).await.map_err(|error| {
                                Problem::Usage(format!(
                                    "Failed to open {path}: {}",
                                    error_text(&error)
                                ))
                            })?
                        }
                        _ => text.into_bytes(),
                    };
                    if matches!(name.as_str(), "-d" | "--data" | "--data-ascii") {
                        piece.retain(|byte| !matches!(byte, b'\n' | b'\r'));
                    }
                    options.json |= name == "--json";
                    options.data.push(piece);
                }
                "--data-urlencode" => {
                    let text = value()?;
                    let piece = if let Some(rest) = text.strip_prefix('=') {
                        percent(rest.as_bytes())
                    } else if let Some((key, rest)) = text.split_once('=') {
                        format!("{key}={}", percent(rest.as_bytes()))
                    } else if let Some((key, path)) = text.split_once('@') {
                        let content = read_file(ctx, path.as_bytes()).await.map_err(|error| {
                            Problem::Usage(format!("Failed to open {path}: {}", error_text(&error)))
                        })?;
                        if key.is_empty() {
                            percent(&content)
                        } else {
                            format!("{key}={}", percent(&content))
                        }
                    } else {
                        percent(text.as_bytes())
                    };
                    options.data.push(piece.into_bytes());
                }
                "-w" | "--write-out" => options.write_out = Some(value()?),
                "-u" | "--user" => options.user = Some(value()?),
                "--oauth2-bearer" => options.bearer = Some(value()?),
                "-A" | "--user-agent" => options.agent = Some(value()?),
                "-e" | "--referer" => options.referer = Some(value()?),
                "-b" | "--cookie" => {
                    let text = value()?;
                    if !text.contains('=') {
                        return Err(Problem::Lacks("a cookie file".to_owned()));
                    }
                    options.cookie = Some(text);
                }
                "-r" | "--range" => options.range = Some(value()?),
                "-D" | "--dump-header" => options.dump_header = Some(value()?.into_bytes()),
                "-T" | "--upload-file" => options.upload = Some(value()?.into_bytes()),
                "--url" => options.urls.push(value()?),
                "--max-redirs" => {
                    let text = value()?;
                    options.redirects = Some(text.parse().map_err(|_| {
                        Problem::Usage(format!(
                            "option {name}: expected a proper numerical parameter"
                        ))
                    })?);
                }
                "-m" | "--max-time" | "--connect-timeout" => {
                    let text = value()?;
                    let seconds: f64 = text.parse().map_err(|_| {
                        Problem::Usage(format!(
                            "option {name}: expected a proper numerical parameter"
                        ))
                    })?;
                    if name != "--connect-timeout" && seconds >= 0.0 {
                        options.timeout = Some(Duration::from_secs_f64(seconds));
                    }
                }
                "--retry"
                | "--retry-delay"
                | "--retry-max-time"
                | "--max-filesize"
                | "--limit-rate"
                | "--cacert"
                | "--cert"
                | "--key"
                | "--resolve"
                | "-x"
                | "--proxy"
                | "--noproxy"
                | "--retry-connrefused"
                | "--retry-all-errors" => {
                    if !matches!(name.as_str(), "--retry-connrefused" | "--retry-all-errors") {
                        value()?;
                    }
                }
                "-F" | "--form" | "--form-string" | "-c" | "--cookie-jar" | "-C"
                | "--continue-at" | "-v" | "--verbose" | "--trace" | "--trace-ascii" | "-K"
                | "--config" | "-Z" | "--parallel" | "-n" | "--netrc" | "--unix-socket"
                | "--digest" | "--ntlm" | "--negotiate" => {
                    return Err(Problem::Lacks(format!("the option {name}")));
                }
                _ => return Err(Problem::Usage(format!("option {name}: is unknown"))),
            }
        }
    }
    Ok(options)
}

fn write_out(
    format: &str,
    response: Option<&Response>,
    request: &Request,
    status: i32,
    message: &str,
    seconds: f64,
) -> String {
    let mut out = String::new();
    let mut rest = format;
    while let Some(at) = rest.find(['%', '\\']) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if let Some(escaped) = tail.strip_prefix('\\') {
            let mut chars = escaped.chars();
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
            rest = chars.as_str();
            continue;
        }
        if let Some(escaped) = tail.strip_prefix("%%") {
            out.push('%');
            rest = escaped;
            continue;
        }
        let Some(close) = tail.strip_prefix("%{").and_then(|inner| inner.find('}')) else {
            out.push('%');
            rest = &tail[1..];
            continue;
        };
        let name = &tail[2..2 + close];
        rest = &tail[3 + close..];
        let text = match name {
            "http_code" | "response_code" => {
                format!("{:03}", response.map_or(0, |found| found.status))
            }
            "size_download" => response.map_or(0, |found| found.body.len()).to_string(),
            "size_upload" => request.body.len().to_string(),
            "url_effective" | "url" => {
                response.map_or_else(|| request.url.clone(), |found| found.url.clone())
            }
            "content_type" => response
                .and_then(|found| header(&found.headers, "content-type"))
                .unwrap_or_default()
                .to_owned(),
            "num_redirects" => response.map_or(0, |found| found.redirects).to_string(),
            "redirect_url" => response
                .filter(|found| (300..400).contains(&found.status))
                .and_then(|found| header(&found.headers, "location"))
                .map(|location| absolute(&request.url, location))
                .unwrap_or_default(),
            "http_version" => response
                .and_then(|found| found.protocol.strip_prefix("HTTP/"))
                .unwrap_or("0")
                .to_owned(),
            "method" => request.method.clone(),
            "scheme" => parts(&request.url)
                .map(|found| found.scheme.to_ascii_uppercase())
                .unwrap_or_default(),
            "exitcode" => status.to_string(),
            "errormsg" => message.to_owned(),
            "time_total" | "time_starttransfer" => format!("{seconds:.6}"),
            "time_namelookup" | "time_connect" | "time_appconnect" | "time_pretransfer"
            | "time_redirect" => "0.000000".to_owned(),
            "stdout" | "stderr" => String::new(),
            _ => String::new(),
        };
        out.push_str(&text);
    }
    out.push_str(rest);
    out
}

async fn curl(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let network = current();
    let options = match curl_options(ctx, argv).await {
        Ok(options) => options,
        Err(Problem::Usage(message)) => {
            ctx.error(&message).await;
            ctx.error("try 'curl --help' or 'curl --manual' for more information")
                .await;
            return 2;
        }
        Err(Problem::Lacks(what)) => {
            ctx.error(&format!("{what} (this simulated shell has no curl for it)"))
                .await;
            return 127;
        }
    };
    if options.urls.is_empty() {
        ctx.error("(2) no URL specified").await;
        ctx.error("try 'curl --help' or 'curl --manual' for more information")
            .await;
        return 2;
    }
    let Some(Network { fetch, limit }) = network else {
        ctx.error(&format!("(7) {UNAVAILABLE}")).await;
        return 7;
    };
    let quiet = options.silent && !options.show_error;
    let mut last = 0;
    for (number, raw) in options.urls.iter().enumerate() {
        let mut url = with_scheme(raw);
        let Some(target_parts) = parts(&url) else {
            if !quiet {
                ctx.error("(3) URL rejected: No host part in the URL").await;
            }
            last = 3;
            continue;
        };
        let mut body = Vec::new();
        for (count, piece) in options.data.iter().enumerate() {
            if count > 0 && !options.json {
                body.push(b'&');
            }
            body.extend_from_slice(piece);
        }
        let mut uploading = false;
        if let Some(path) = &options.upload {
            match read_file(ctx, path).await {
                Ok(content) => {
                    body = content;
                    uploading = true;
                }
                Err(error) => {
                    if !quiet {
                        ctx.error(&format!(
                            "(26) Failed to open/read local data from file/application: {}",
                            error_text(&error)
                        ))
                        .await;
                    }
                    last = 26;
                    continue;
                }
            }
        }
        let has_data = !options.data.is_empty();
        if options.query && has_data {
            let joiner = if url.contains('?') { '&' } else { '?' };
            url.push(joiner);
            url.push_str(&String::from_utf8_lossy(&body));
            body.clear();
        }
        let method = match &options.method {
            Some(method) => method.clone(),
            None if options.head => "HEAD".to_owned(),
            None if uploading => "PUT".to_owned(),
            None if has_data && !options.query => "POST".to_owned(),
            None => "GET".to_owned(),
        };
        let mut headers = vec![
            (
                "Host".to_owned(),
                match (target_parts.scheme.as_str(), target_parts.port) {
                    ("http", 80) | ("https", 443) => target_parts.host.clone(),
                    (_, port) => format!("{}:{port}", target_parts.host),
                },
            ),
            (
                "User-Agent".to_owned(),
                options.agent.clone().unwrap_or_else(|| AGENT.to_owned()),
            ),
            ("Accept".to_owned(), "*/*".to_owned()),
        ];
        if options.json {
            set_header(&mut headers, "Content-Type", "application/json");
            set_header(&mut headers, "Accept", "application/json");
        } else if has_data && !options.query {
            set_header(
                &mut headers,
                "Content-Type",
                "application/x-www-form-urlencoded",
            );
        }
        if let Some(user) = &options.user {
            set_header(
                &mut headers,
                "Authorization",
                &format!("Basic {}", base64(user.as_bytes())),
            );
        }
        if let Some(token) = &options.bearer {
            set_header(&mut headers, "Authorization", &format!("Bearer {token}"));
        }
        if let Some(referer) = &options.referer {
            set_header(&mut headers, "Referer", referer);
        }
        if let Some(cookie) = &options.cookie {
            set_header(&mut headers, "Cookie", cookie);
        }
        if let Some(range) = &options.range {
            set_header(&mut headers, "Range", &format!("bytes={range}"));
        }
        if options.compressed {
            set_header(&mut headers, "Accept-Encoding", "deflate, gzip");
        }
        for line in &options.headers {
            match line.split_once(':') {
                Some((name, value)) if value.trim().is_empty() => {
                    headers.retain(|(key, _)| !key.eq_ignore_ascii_case(name.trim()));
                }
                Some((name, value)) => set_header(&mut headers, name.trim(), value.trim()),
                None => {
                    if let Some(name) = line.strip_suffix(';') {
                        set_header(&mut headers, name.trim(), "");
                    }
                }
            }
        }
        let request = Request {
            method,
            url: url.clone(),
            headers,
            body: Bytes::from(body),
            follow: options
                .follow
                .then(|| options.redirects.unwrap_or(CURL_REDIRECTS)),
            timeout: options.timeout,
            limit,
        };
        let started = ctx.now();
        let answer = fetch.fetch(request.clone()).await;
        let seconds = ctx.now().saturating_sub(started).as_secs_f64();
        let answer = match answer {
            Ok(response) if response.body.len() as u64 > limit => Err(Failure::TooLarge),
            other => other,
        };
        let mut status = 0;
        let mut message = String::new();
        let response = match answer {
            Ok(response) => Some(response),
            Err(failure) => {
                (status, message) = curl_failure(
                    &failure,
                    &url,
                    options.timeout,
                    options.redirects.unwrap_or(CURL_REDIRECTS),
                );
                None
            }
        };
        if let Some(response) = &response {
            if options.fail && response.status >= 400 {
                status = 22;
                message = format!("The requested URL returned error: {}", response.status);
            }
            if let Some(path) = &options.dump_header
                && let Err(error) = write_file(ctx, path, &head_text(response)).await
            {
                status = 23;
                message = format!("Failed writing header: {}", error_text(&error));
            }
        }
        if let (Some(response), true) = (&response, status == 0) {
            let mut output = Vec::new();
            if options.include || options.head {
                output.extend_from_slice(&head_text(response));
            }
            if !options.head {
                output.extend_from_slice(&response.body);
            }
            let written = match options.targets.get(number).unwrap_or(&Target::Stdout) {
                Target::Stdout => ctx.write(&output).await,
                Target::File(path) => write_file(ctx, path, &output).await,
                Target::Remote => {
                    let name = target_parts.path.rsplit('/').next().unwrap_or_default();
                    if name.is_empty() {
                        Err(std::io::Error::other("Remote file name has no length"))
                    } else {
                        write_file(ctx, name.as_bytes(), &output).await
                    }
                }
            };
            if let Err(error) = written {
                if error.kind() == std::io::ErrorKind::BrokenPipe {
                    return 141;
                }
                status = 23;
                message = format!(
                    "Failure writing output to destination: {}",
                    error_text(&error)
                );
            }
        }
        if let Some(format) = &options.write_out {
            let text = write_out(
                format,
                response.as_ref(),
                &request,
                status,
                &message,
                seconds,
            );
            if ctx.write(text.as_bytes()).await.is_err() {
                return 141;
            }
        }
        if status != 0 && !quiet {
            let _ = ctx.flush().await;
            ctx.error(&format!("({status}) {message}")).await;
        }
        last = status;
    }
    last
}

#[derive(Default)]
struct Wget {
    urls: Vec<String>,
    output: Option<Vec<u8>>,
    directory: Option<Vec<u8>>,
    quiet: bool,
    brief: bool,
    spider: bool,
    no_clobber: bool,
    server_response: bool,
    preemptive: bool,
    headers: Vec<String>,
    method: Option<String>,
    body: Option<Vec<u8>>,
    agent: Option<String>,
    user: Option<String>,
    password: Option<String>,
    timeout: Option<Duration>,
    redirects: u32,
}

async fn wget_options(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> Result<Wget, Problem> {
    let mut options = Wget {
        redirects: WGET_REDIRECTS,
        ..Wget::default()
    };
    let mut args = Vec::new();
    for arg in &argv[1..] {
        match std::str::from_utf8(arg) {
            Ok(text) => args.push(text.to_owned()),
            Err(_) => return Err(Problem::Usage("an argument is not UTF-8".to_owned())),
        }
    }
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].clone();
        index += 1;
        if !arg.starts_with('-') || arg == "-" {
            options.urls.push(arg);
            continue;
        }
        let (name, attached) = if let Some(long) = arg.strip_prefix("--") {
            match long.split_once('=') {
                Some((name, value)) => (format!("--{name}"), Some(value.to_owned())),
                None => (arg.clone(), None),
            }
        } else if arg == "-nv" || arg == "-nc" || arg == "-nd" || arg == "-nH" {
            (arg.clone(), None)
        } else {
            let rest = &arg[2..];
            (
                arg[..2].to_owned(),
                (!rest.is_empty()).then(|| rest.to_owned()),
            )
        };
        let mut value = || -> Result<String, Problem> {
            if let Some(value) = attached.clone() {
                return Ok(value);
            }
            let value = args.get(index).cloned().ok_or_else(|| {
                Problem::Usage(format!(
                    "option requires an argument -- '{}'",
                    name.trim_start_matches('-')
                ))
            })?;
            index += 1;
            Ok(value)
        };
        match name.as_str() {
            "-q" | "--quiet" => options.quiet = true,
            "-nv" | "--no-verbose" => options.brief = true,
            "--spider" => options.spider = true,
            "-nc" | "--no-clobber" => options.no_clobber = true,
            "-S" | "--server-response" => options.server_response = true,
            "--auth-no-challenge" => options.preemptive = true,
            "-c"
            | "--continue"
            | "--no-check-certificate"
            | "-nd"
            | "-nH"
            | "--content-disposition"
            | "--no-cache"
            | "--no-cookies"
            | "--show-progress"
            | "--no-hsts"
            | "-4"
            | "-6"
            | "--https-only"
            | "-v"
            | "--verbose" => {}
            "-O" | "--output-document" => options.output = Some(value()?.into_bytes()),
            "-P" | "--directory-prefix" => options.directory = Some(value()?.into_bytes()),
            "--header" => options.headers.push(value()?),
            "--method" => options.method = Some(value()?),
            "--post-data" | "--body-data" => {
                options.body = Some(value()?.into_bytes());
                if name == "--post-data" && options.method.is_none() {
                    options.method = Some("POST".to_owned());
                }
            }
            "--post-file" | "--body-file" => {
                let path = value()?;
                let content = read_file(ctx, path.as_bytes())
                    .await
                    .map_err(|error| Problem::Usage(format!("{path}: {}", error_text(&error))))?;
                options.body = Some(content);
                if name == "--post-file" && options.method.is_none() {
                    options.method = Some("POST".to_owned());
                }
            }
            "-U" | "--user-agent" => options.agent = Some(value()?),
            "--user" | "--http-user" => options.user = Some(value()?),
            "--password" | "--http-password" => options.password = Some(value()?),
            "-T" | "--timeout" | "--read-timeout" => {
                let seconds: f64 = value()?
                    .parse()
                    .map_err(|_| Problem::Usage(format!("{name}: Invalid time period")))?;
                options.timeout = (seconds > 0.0).then(|| Duration::from_secs_f64(seconds));
            }
            "--max-redirect" => {
                options.redirects = value()?
                    .parse()
                    .map_err(|_| Problem::Usage(format!("{name}: Invalid number")))?;
            }
            "-t" | "--tries" | "-w" | "--wait" | "--connect-timeout" | "--dns-timeout"
            | "--waitretry" | "--progress" | "-e" | "--execute" => {
                value()?;
            }
            "-r" | "--recursive" | "-i" | "--input-file" | "-m" | "--mirror" | "-N"
            | "--timestamping" | "-o" | "--output-file" | "-a" | "--append-output" | "-b"
            | "--background" | "-k" | "--convert-links" | "-p" | "--page-requisites"
            | "--load-cookies" | "--save-cookies" => {
                return Err(Problem::Lacks(format!("the option {name}")));
            }
            _ => return Err(Problem::Usage(format!("unrecognized option '{name}'"))),
        }
    }
    Ok(options)
}

fn stamp(ctx: &UtilCtx<'_>) -> String {
    let now = ctx.now().as_secs() as i64;
    let local = now + i64::from(ctx.utc_offset(now));
    let days = local.div_euclid(86_400);
    let second = local.rem_euclid(86_400);
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        second / 3_600,
        second / 60 % 60,
        second % 60
    )
}

async fn wget(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let network = current();
    let options = match wget_options(ctx, argv).await {
        Ok(options) => options,
        Err(Problem::Usage(message)) => {
            ctx.error(&message).await;
            ctx.error_raw(
                b"Usage: wget [OPTION]... [URL]...\n\nTry `wget --help' for more options.\n",
            )
            .await;
            return 2;
        }
        Err(Problem::Lacks(what)) => {
            ctx.error(&format!("{what} (this simulated shell has no wget for it)"))
                .await;
            return 127;
        }
    };
    if options.urls.is_empty() {
        ctx.error("missing URL").await;
        ctx.error_raw(b"Usage: wget [OPTION]... [URL]...\n\nTry `wget --help' for more options.\n")
            .await;
        return 1;
    }
    let Some(Network { fetch, limit }) = network else {
        ctx.error(UNAVAILABLE).await;
        return 4;
    };
    let chatty = !options.quiet && !options.brief;
    let mut last = 0;
    for raw in &options.urls {
        let url = with_scheme(raw);
        let Some(target_parts) = parts(&url) else {
            ctx.error_raw(format!("{raw}: Invalid host name.\n").as_bytes())
                .await;
            last = 1;
            continue;
        };
        let mut headers = vec![
            (
                "Host".to_owned(),
                match (target_parts.scheme.as_str(), target_parts.port) {
                    ("http", 80) | ("https", 443) => target_parts.host.clone(),
                    (_, port) => format!("{}:{port}", target_parts.host),
                },
            ),
            (
                "User-Agent".to_owned(),
                options
                    .agent
                    .clone()
                    .unwrap_or_else(|| WGET_AGENT.to_owned()),
            ),
            ("Accept".to_owned(), "*/*".to_owned()),
            ("Accept-Encoding".to_owned(), "identity".to_owned()),
        ];
        let credentials = options.user.as_ref().map(|user| {
            let pair = format!("{user}:{}", options.password.clone().unwrap_or_default());
            format!("Basic {}", base64(pair.as_bytes()))
        });
        if let (Some(credentials), true) = (&credentials, options.preemptive) {
            set_header(&mut headers, "Authorization", credentials);
        }
        if options.body.is_some() {
            set_header(
                &mut headers,
                "Content-Type",
                "application/x-www-form-urlencoded",
            );
        }
        for line in &options.headers {
            if let Some((name, value)) = line.split_once(':') {
                set_header(&mut headers, name.trim(), value.trim());
            }
        }
        let method = match &options.method {
            Some(method) => method.clone(),
            None if options.spider => "HEAD".to_owned(),
            None => "GET".to_owned(),
        };
        if chatty {
            ctx.error_raw(format!("--{}--  {url}\n", stamp(ctx)).as_bytes())
                .await;
        }
        let request = Request {
            method,
            url: url.clone(),
            headers,
            body: Bytes::from(options.body.clone().unwrap_or_default()),
            follow: Some(options.redirects),
            timeout: options.timeout,
            limit,
        };
        let mut answer = fetch.fetch(request.clone()).await;
        if let (Ok(response), Some(credentials), false) =
            (&answer, &credentials, options.preemptive)
            && response.status == 401
        {
            let mut again = request;
            set_header(&mut again.headers, "Authorization", credentials);
            answer = fetch.fetch(again).await;
        }
        let response = match answer {
            Ok(response) if response.body.len() as u64 > limit => {
                ctx.error("the answer is larger than this shell keeps")
                    .await;
                last = 3;
                continue;
            }
            Ok(response) => response,
            Err(failure) => {
                let (status, text) = match failure {
                    Failure::Resolve => (
                        4,
                        format!("unable to resolve host address '{}'", target_parts.host),
                    ),
                    Failure::Connect => (
                        4,
                        format!(
                            "failed: Connection refused ({}:{})",
                            target_parts.host, target_parts.port
                        ),
                    ),
                    Failure::Timeout => (4, "Read error (Connection timed out)".to_owned()),
                    Failure::Redirects => {
                        (8, format!("{} redirections exceeded.", options.redirects))
                    }
                    Failure::Secure(detail) => (5, detail),
                    Failure::TooLarge => {
                        (3, "the answer is larger than this shell keeps".to_owned())
                    }
                    Failure::Denied(detail) => (4, format!("{detail}: {UNAVAILABLE}")),
                    Failure::Other(detail) => (4, detail),
                };
                ctx.error(&text).await;
                last = status;
                continue;
            }
        };
        if options.server_response && !options.quiet {
            let mut lines = format!(
                "  {} {} {}\n",
                response.protocol, response.status, response.reason
            );
            for (name, value) in &response.headers {
                lines.push_str(&format!("  {name}: {value}\n"));
            }
            ctx.error_raw(lines.as_bytes()).await;
        }
        if chatty {
            ctx.error_raw(
                format!(
                    "HTTP request sent, awaiting response... {} {}\n",
                    response.status, response.reason
                )
                .as_bytes(),
            )
            .await;
        }
        if response.status >= 400 {
            if !options.quiet {
                ctx.error_raw(
                    format!(
                        "{} ERROR {}: {}.\n",
                        stamp(ctx),
                        response.status,
                        response.reason
                    )
                    .as_bytes(),
                )
                .await;
            }
            last = 8;
            continue;
        }
        if options.spider {
            continue;
        }
        let kind = header(&response.headers, "content-type").unwrap_or("unknown");
        let to_stdout = options.output.as_deref() == Some(b"-".as_slice());
        let mut name = match &options.output {
            Some(path) => path.clone(),
            None => {
                let landed = parts(&response.url)
                    .map_or_else(|| target_parts.path.clone(), |found| found.path);
                let base = landed.rsplit('/').next().unwrap_or_default();
                let base = if base.is_empty() { "index.html" } else { base };
                match &options.directory {
                    Some(directory) => crate::fs::join(directory, base.as_bytes()),
                    None => base.as_bytes().to_vec(),
                }
            }
        };
        if options.output.is_none() && ctx.stat(&name, true).await.is_ok() {
            if options.no_clobber {
                if !options.quiet {
                    ctx.error_raw(
                        format!(
                            "File '{}' already there; not retrieving.\n",
                            String::from_utf8_lossy(&name)
                        )
                        .as_bytes(),
                    )
                    .await;
                }
                continue;
            }
            let mut count = 1;
            loop {
                let mut numbered = name.clone();
                numbered.extend_from_slice(format!(".{count}").as_bytes());
                if ctx.stat(&numbered, true).await.is_err() {
                    name = numbered;
                    break;
                }
                count += 1;
            }
        }
        let shown = if to_stdout {
            "STDOUT".to_owned()
        } else {
            String::from_utf8_lossy(&name).into_owned()
        };
        if chatty {
            ctx.error_raw(
                format!(
                    "Length: {} [{kind}]\nSaving to: '{shown}'\n\n",
                    response.body.len()
                )
                .as_bytes(),
            )
            .await;
        }
        let written = if to_stdout {
            ctx.write(&response.body).await
        } else {
            if let Some(directory) = &options.directory
                && options.output.is_none()
            {
                let _ = ctx.mkdir(directory, 0o755).await;
            }
            write_file(ctx, &name, &response.body).await
        };
        if let Err(error) = written {
            if error.kind() == std::io::ErrorKind::BrokenPipe {
                return 141;
            }
            ctx.error(&format!("{shown}: {}", error_text(&error))).await;
            last = 3;
            continue;
        }
        if !options.quiet {
            let size = response.body.len();
            let line = if options.brief {
                format!(
                    "{} URL:{} [{size}/{size}] -> \"{shown}\" [1]\n",
                    stamp(ctx),
                    response.url
                )
            } else {
                format!("{} - '{shown}' saved [{size}/{size}]\n\n", stamp(ctx))
            };
            ctx.error_raw(line.as_bytes()).await;
        }
    }
    last
}
