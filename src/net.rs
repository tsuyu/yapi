use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use crate::model::{is_loopback, urlencode_pairs, BodyKind, RequestSpec};
use reqwest::blocking::multipart;

#[derive(Debug, Clone, Copy)]
pub struct SendOpts {
    pub timeout_secs: u64,
    /// Skip certificate validation. For https dev servers with a self-signed
    /// cert; never leave it on against anything you do not control.
    pub insecure_tls: bool,
}

/// What the response body actually is, from the content type with a sniff as
/// backup - servers lie, or say nothing at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Json,
    Xml,
    Html,
    Text,
    Image,
    Binary,
}

impl Shape {
    pub fn as_str(self) -> &'static str {
        match self {
            Shape::Json => "json",
            Shape::Xml => "xml",
            Shape::Html => "html",
            Shape::Text => "text",
            Shape::Image => "image",
            Shape::Binary => "binary",
        }
    }

    pub fn is_text(self) -> bool {
        !matches!(self, Shape::Image | Shape::Binary)
    }
}

#[derive(Debug)]
pub struct ResponseData {
    pub status: u16,
    pub status_text: String,
    pub elapsed_ms: u128,
    pub size: usize,
    pub headers: Vec<(String, String)>,
    /// Decoded text, empty for binary bodies.
    pub body: String,
    pub pretty: Option<String>,
    pub final_url: String,
    pub content_type: String,
    pub shape: Shape,
    /// The bytes exactly as received, for image preview and save-to-file.
    pub bytes: Vec<u8>,
}

impl ResponseData {
    /// Set-Cookie headers, split into name, value and attributes.
    pub fn cookies(&self) -> Vec<(String, String, String)> {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
            .filter_map(|(_, v)| {
                let mut parts = v.split(';');
                let (name, value) = parts.next()?.trim().split_once('=')?;
                let attrs = parts
                    .map(str::trim)
                    .filter(|a| !a.is_empty())
                    .collect::<Vec<_>>()
                    .join("; ");
                Some((name.trim().to_owned(), value.trim().to_owned(), attrs))
            })
            .collect()
    }
}

/// Classify a body. `content_type` wins when it is specific; otherwise sniff.
pub fn detect_shape(content_type: &str, bytes: &[u8]) -> Shape {
    let ct = content_type.to_ascii_lowercase();
    let ct = ct.split(';').next().unwrap_or("").trim();
    let by_type = match ct {
        "" | "application/octet-stream" => None,
        t if t.starts_with("image/") => Some(Shape::Image),
        t if t.contains("json") => Some(Shape::Json),
        t if t.contains("html") => Some(Shape::Html),
        t if t.contains("xml") => Some(Shape::Xml),
        t if t.starts_with("text/") => Some(Shape::Text),
        t if t.starts_with("audio/") || t.starts_with("video/") => Some(Shape::Binary),
        t if t.starts_with("application/")
            && (t.contains("pdf") || t.contains("zip") || t.contains("octet")) =>
        {
            Some(Shape::Binary)
        }
        _ => None,
    };
    if let Some(shape) = by_type {
        return shape;
    }

    // magic numbers for the image formats we can decode
    let magic = [
        (&b"\x89PNG"[..], Shape::Image),
        (&b"\xff\xd8\xff"[..], Shape::Image),
        (&b"GIF8"[..], Shape::Image),
        (&b"BM"[..], Shape::Image),
        (&b"RIFF"[..], Shape::Image),
    ];
    if magic.iter().any(|(sig, _)| bytes.starts_with(sig)) {
        return Shape::Image;
    }

    match std::str::from_utf8(bytes) {
        Err(_) => Shape::Binary,
        Ok(text) => {
            let t = text.trim_start();
            if t.starts_with('{') || t.starts_with('[') {
                Shape::Json
            } else if t.to_ascii_lowercase().starts_with("<!doctype html")
                || t.to_ascii_lowercase().starts_with("<html")
            {
                Shape::Html
            } else if t.starts_with("<?xml") || t.starts_with('<') {
                Shape::Xml
            } else if text.bytes().filter(|b| *b == 0).count() > 0 {
                Shape::Binary
            } else {
                Shape::Text
            }
        }
    }
}

pub enum Msg {
    Done(Box<ResponseData>),
    Failed(String),
}

/// Fire the request on a worker thread; result arrives on `tx`.
pub fn spawn(spec: RequestSpec, opts: SendOpts, tx: Sender<Msg>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let msg = match run(spec, opts) {
            Ok(r) => Msg::Done(Box::new(r)),
            Err(e) => Msg::Failed(e),
        };
        let _ = tx.send(msg);
        ctx.request_repaint();
    });
}

/// Send a request on the calling thread. Used by the chain runner, which is
/// already on a worker thread of its own.
pub fn run_blocking(spec: RequestSpec, opts: SendOpts) -> Result<ResponseData, String> {
    run(spec, opts)
}

fn run(spec: RequestSpec, opts: SendOpts) -> Result<ResponseData, String> {
    let url = spec.full_url();
    if url.is_empty() {
        return Err("url is empty".to_owned());
    }
    let client = client_with(opts, &url, &spec.transport)?;

    let method = reqwest::Method::from_bytes(spec.method.as_str().as_bytes())
        .map_err(|e| format!("bad method: {e}"))?;
    let mut req = client.request(method, &url);

    // auth first, but an explicit header of the same name always wins
    if let Some((k, v)) = spec.auth.header() {
        let overridden = spec
            .headers
            .iter()
            .any(|h| h.active() && h.key.trim().eq_ignore_ascii_case(&k));
        if !overridden {
            req = req.header(k, v);
        }
    }

    for h in spec.headers.iter().filter(|h| h.active()) {
        req = req.header(h.key.trim(), h.value.as_str());
    }

    if let Some(cookies) = spec.cookie_header() {
        if !has_header(&spec, "cookie") {
            req = req.header(reqwest::header::COOKIE, cookies);
        }
    }

    // an explicit Content-Type row always wins over the body kind default
    let ct_set = has_header(&spec, "content-type");
    if let Some(ct) = spec.body_kind.content_type() {
        if !ct_set {
            req = req.header(reqwest::header::CONTENT_TYPE, ct);
        }
    }

    match spec.body_kind {
        BodyKind::None => {}
        BodyKind::Json | BodyKind::Xml | BodyKind::Text => {
            req = req.body(spec.body.clone());
        }
        BodyKind::Form => {
            let pairs = crate::model::parse_query(&spec.body.replace(['\n', '\r'], "&"));
            req = req.body(urlencode_pairs(&pairs));
        }
        BodyKind::Multipart => {
            // reqwest sets Content-Type itself here - it carries the boundary
            req = req.multipart(build_multipart(&spec)?);
        }
        BodyKind::Binary => {
            let path = spec.binary_path.trim();
            if path.is_empty() {
                return Err("binary body selected but no file chosen".to_owned());
            }
            let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
            if !ct_set {
                let ct = if spec.binary_content_type.trim().is_empty() {
                    guess_content_type(path).to_owned()
                } else {
                    spec.binary_content_type.trim().to_owned()
                };
                req = req.header(reqwest::header::CONTENT_TYPE, ct);
            }
            req = req.body(bytes);
        }
    }

    let start = Instant::now();
    let resp = req.send().map_err(|e| explain(&e, &url))?;
    let elapsed_ms = start.elapsed().as_millis();

    let status = resp.status();
    let final_url = resp.url().to_string();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("<binary>").to_owned()))
        .collect();
    let content_type = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();

    let bytes = resp.bytes().map_err(|e| e.to_string())?.to_vec();
    let shape = detect_shape(&content_type, &bytes);
    let body = if shape.is_text() {
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        String::new()
    };
    let pretty = match shape {
        Shape::Json => serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok()),
        Shape::Xml | Shape::Html => Some(crate::pretty::xml(&body)),
        _ => None,
    };

    Ok(ResponseData {
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or("").to_owned(),
        elapsed_ms,
        size: bytes.len(),
        headers,
        body,
        pretty,
        final_url,
        content_type,
        shape,
        bytes,
    })
}

/// A client configured for one target: proxies bypassed for loopback, and cert
/// checks optionally skipped.
pub fn client(opts: SendOpts, url: &str) -> Result<reqwest::blocking::Client, String> {
    client_with(opts, url, &crate::model::Transport::default())
}

/// The same, plus the per-request transport options (redirects, proxy, certs).
pub fn client_with(
    opts: SendOpts,
    url: &str,
    transport: &crate::model::Transport,
) -> Result<reqwest::blocking::Client, String> {
    let mut builder = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(opts.timeout_secs.max(1)))
        .user_agent("api-req/0.1");

    builder = builder.redirect(if transport.follow_redirects {
        reqwest::redirect::Policy::limited(transport.max_redirects)
    } else {
        reqwest::redirect::Policy::none()
    });

    if !transport.compressed {
        builder = builder.no_gzip().no_brotli().no_deflate();
    }

    let proxy = transport.proxy.trim();
    if !proxy.is_empty() {
        builder = builder.proxy(
            reqwest::Proxy::all(crate::model::normalize_url(proxy))
                .map_err(|e| format!("bad proxy {proxy:?}: {e}"))?,
        );
    } else if is_loopback(url) {
        // a system/corporate proxy must never swallow a request to this machine
        builder = builder.no_proxy();
    }

    let ca = transport.ca_cert.trim();
    if !ca.is_empty() {
        let pem = std::fs::read(ca).map_err(|e| format!("{ca}: {e}"))?;
        for cert in reqwest::Certificate::from_pem_bundle(&pem)
            .map_err(|e| format!("{ca}: {e}"))?
        {
            builder = builder.add_root_certificate(cert);
        }
    }

    let cert = transport.client_cert.trim();
    if !cert.is_empty() {
        // rustls wants the key and the certificate in one pem blob
        let mut pem = std::fs::read(cert).map_err(|e| format!("{cert}: {e}"))?;
        let key = transport.client_key.trim();
        if !key.is_empty() {
            pem.push(b'\n');
            pem.extend(std::fs::read(key).map_err(|e| format!("{key}: {e}"))?);
        }
        builder = builder.identity(
            reqwest::Identity::from_pem(&pem).map_err(|e| format!("{cert}: {e}"))?,
        );
    }

    if opts.insecure_tls {
        builder = builder.danger_accept_invalid_certs(true);
    }
    builder.build().map_err(|e| e.to_string())
}

fn has_header(spec: &RequestSpec, name: &str) -> bool {
    spec.headers
        .iter()
        .any(|h| h.active() && h.key.trim().eq_ignore_ascii_case(name))
}

fn build_multipart(spec: &RequestSpec) -> Result<multipart::Form, String> {
    let mut form = multipart::Form::new();
    for part in spec.form_parts.iter().filter(|p| p.active()) {
        let key = part.key.trim().to_owned();
        if part.is_file() {
            let path = part.file.trim();
            let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
            let file_name = std::path::Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".to_owned());
            let ct = if part.content_type.trim().is_empty() {
                guess_content_type(path).to_owned()
            } else {
                part.content_type.trim().to_owned()
            };
            let file_part = multipart::Part::bytes(bytes)
                .file_name(file_name)
                .mime_str(&ct)
                .map_err(|e| format!("bad content-type {ct:?}: {e}"))?;
            form = form.part(key, file_part);
        } else if part.content_type.trim().is_empty() {
            form = form.text(key, part.value.clone());
        } else {
            let text_part = multipart::Part::text(part.value.clone())
                .mime_str(part.content_type.trim())
                .map_err(|e| format!("bad content-type: {e}"))?;
            form = form.part(key, text_part);
        }
    }
    Ok(form)
}

/// Content type from the file extension, for file uploads.
pub fn guess_content_type(path: &str) -> &'static str {
    let ext = std::path::Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "json" => "application/json",
        "xml" => "application/xml",
        "txt" | "log" | "md" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "js" => "application/javascript",
        "css" => "text/css",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// reqwest errors are terse; add the hint that usually applies locally.
fn explain(err: &reqwest::Error, url: &str) -> String {
    let mut msg = err.to_string();
    let mut src: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(err);
    while let Some(e) = src {
        msg.push_str(&format!(": {e}"));
        src = e.source();
    }
    let low = msg.to_ascii_lowercase();
    let hint = if low.contains("refused") {
        if is_loopback(url) {
            Some("nothing is listening there - is the dev server up, and on that port?")
        } else {
            Some("connection refused by the host")
        }
    } else if low.contains("certificate") || low.contains("self-signed") || low.contains("tls") {
        Some("self-signed cert? tick 'insecure tls' in the top bar")
    } else if low.contains("dns") || low.contains("resolve") {
        Some("host did not resolve - typo, or try 127.0.0.1 instead of localhost")
    } else if err.is_timeout() {
        Some("timed out - raise the timeout in the top bar")
    } else {
        None
    };
    match hint {
        Some(h) => format!("{msg}  ({h})"),
        None => msg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Auth, AuthKind, BodyKind, KeyVal, Method};
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    /// One-shot server answering with a caller-chosen content type and body.
    fn serving(content_type: &'static str, body: &'static [u8], extra: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
            }
            let mut out = &stream;
            write!(
                out,
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            out.write_all(body).unwrap();
            out.flush().unwrap();
        });
        port
    }

    fn get(port: u16) -> ResponseData {
        run(
            RequestSpec {
                url: format!("localhost:{port}/"),
                params: vec![],
                headers: vec![],
                ..Default::default()
            },
            SendOpts {
                timeout_secs: 10,
                insecure_tls: false,
            },
        )
        .expect("request failed")
    }

    #[test]
    fn xml_response_is_classified_and_indented() {
        let port = serving("application/xml", b"<rss><item>hi</item></rss>", "");
        let resp = get(port);
        assert_eq!(resp.shape, Shape::Xml);
        assert_eq!(
            resp.pretty.as_deref(),
            Some("<rss>\n  <item>\n    hi\n  </item>\n</rss>")
        );
        assert_eq!(resp.size, 26);
    }

    #[test]
    fn image_response_keeps_its_bytes_and_skips_text() {
        // a 1x1 png
        const PNG: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52,
        ];
        let port = serving("image/png", PNG, "");
        let resp = get(port);
        assert_eq!(resp.shape, Shape::Image);
        assert_eq!(resp.bytes, PNG);
        assert!(resp.body.is_empty(), "binary body should not be decoded as text");
        assert!(resp.pretty.is_none());
    }

    #[test]
    fn response_cookies_come_back_split() {
        let port = serving(
            "text/plain",
            b"ok",
            "Set-Cookie: sid=xyz; Path=/; HttpOnly\r\nSet-Cookie: lang=en\r\n",
        );
        let resp = get(port);
        let cookies = resp.cookies();
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies[0].0, "sid");
        assert_eq!(cookies[0].1, "xyz");
        assert!(cookies[0].2.contains("HttpOnly"));
    }

    #[test]
    fn json_response_keeps_key_order_when_pretty_printed() {
        let port = serving("application/json", br#"{"zeta":1,"alpha":2}"#, "");
        let resp = get(port);
        assert_eq!(resp.shape, Shape::Json);
        let pretty = resp.pretty.unwrap();
        assert!(
            pretty.find("zeta").unwrap() < pretty.find("alpha").unwrap(),
            "keys were reordered:\n{pretty}"
        );
    }

    /// Minimal one-shot http server on a loopback port. Returns the port and a
    /// handle yielding the raw request it saw.
    fn one_shot_server() -> (u16, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut request = String::new();
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                if let Some(v) = line
                    .to_ascii_lowercase()
                    .strip_prefix("content-length:")
                {
                    content_length = v.trim().parse().unwrap_or(0);
                }
                let done = line == "\r\n" || line == "\n";
                request.push_str(&line);
                if done {
                    break;
                }
            }
            if content_length > 0 {
                let mut body = vec![0u8; content_length];
                std::io::Read::read_exact(&mut reader, &mut body).unwrap();
                request.push_str(&String::from_utf8_lossy(&body));
            }
            let body = br#"{"ok":true}"#;
            let mut stream = &stream;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
            stream.flush().unwrap();
            request
        });
        (port, handle)
    }

    #[test]
    fn hits_a_localhost_server_without_a_scheme() {
        let (port, server) = one_shot_server();
        let spec = RequestSpec {
            // no scheme, no https - exactly what you type for a dev server
            url: format!("localhost:{port}/api/items"),
            params: vec![KeyVal {
                on: true,
                key: "page".into(),
                value: "2".into(),
            }],
            headers: vec![KeyVal::default()],
            method: Method::Get,
            auth: Auth {
                kind: AuthKind::Bearer,
                token: "tok123".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };

        let resp = run(
            spec,
            SendOpts {
                timeout_secs: 10,
                insecure_tls: false,
            },
        )
        .expect("request to localhost should succeed");

        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, r#"{"ok":true}"#);
        assert!(resp.pretty.is_some());

        let seen = server.join().unwrap();
        assert!(
            seen.starts_with("GET /api/items?page=2 HTTP/1.1"),
            "unexpected request line in:\n{seen}"
        );
        assert!(
            seen.to_ascii_lowercase().contains("authorization: bearer tok123"),
            "auth header missing from:\n{seen}"
        );
    }


    fn send_to(spec: RequestSpec) -> (ResponseData, String) {
        let resp = run(
            spec,
            SendOpts {
                timeout_secs: 10,
                insecure_tls: false,
            },
        );
        (resp.expect("request failed"), String::new())
    }

    #[test]
    fn path_params_and_cookies_reach_the_wire() {
        let (port, server) = one_shot_server();
        let spec = RequestSpec {
            url: format!("localhost:{port}/users/:id/posts/{{postId}}"),
            path_params: vec![
                KeyVal {
                    on: true,
                    key: "id".into(),
                    value: "42".into(),
                },
                KeyVal {
                    on: true,
                    key: "postId".into(),
                    value: "a b".into(),
                },
            ],
            cookies: vec![
                KeyVal {
                    on: true,
                    key: "session".into(),
                    value: "abc123".into(),
                },
                KeyVal {
                    on: true,
                    key: "theme".into(),
                    value: "dark".into(),
                },
                KeyVal::default(),
            ],
            params: vec![],
            headers: vec![],
            ..Default::default()
        };
        let _ = send_to(spec);
        let seen = server.join().unwrap();
        assert!(
            seen.starts_with("GET /users/42/posts/a%20b HTTP/1.1"),
            "path params not substituted:\n{seen}"
        );
        assert!(
            seen.to_ascii_lowercase()
                .contains("cookie: session=abc123; theme=dark"),
            "cookie header missing:\n{seen}"
        );
    }

    #[test]
    fn multipart_sends_text_fields_and_files() {
        let dir = std::env::temp_dir().join(format!("api-req-mp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hello.json");
        std::fs::write(&file, br#"{"from":"file"}"#).unwrap();

        let (port, server) = one_shot_server();
        let spec = RequestSpec {
            method: Method::Post,
            url: format!("localhost:{port}/upload"),
            body_kind: BodyKind::Multipart,
            form_parts: vec![
                crate::model::FormPart {
                    on: true,
                    key: "title".into(),
                    value: "a caption".into(),
                    file: String::new(),
                    content_type: String::new(),
                },
                crate::model::FormPart {
                    on: true,
                    key: "attachment".into(),
                    value: String::new(),
                    file: file.display().to_string(),
                    content_type: String::new(),
                },
                crate::model::FormPart::default(),
            ],
            params: vec![],
            headers: vec![],
            ..Default::default()
        };
        let _ = send_to(spec);
        let seen = server.join().unwrap();

        assert!(
            seen.to_ascii_lowercase()
                .contains("content-type: multipart/form-data; boundary="),
            "no multipart content type:\n{seen}"
        );
        assert!(seen.contains(r#"name="title""#), "text part missing:\n{seen}");
        assert!(seen.contains("a caption"), "text value missing:\n{seen}");
        assert!(
            seen.contains(r#"name="attachment""#) && seen.contains(r#"filename="hello.json""#),
            "file part missing:\n{seen}"
        );
        assert!(
            seen.contains("application/json"),
            "file content type not guessed from the extension:\n{seen}"
        );
        assert!(
            seen.contains(r#"{"from":"file"}"#),
            "file contents missing:\n{seen}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn binary_body_sends_the_file_bytes() {
        let dir = std::env::temp_dir().join(format!("api-req-bin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("payload.xml");
        std::fs::write(&file, b"<note>hi</note>").unwrap();

        let (port, server) = one_shot_server();
        let spec = RequestSpec {
            method: Method::Put,
            url: format!("localhost:{port}/blob"),
            body_kind: BodyKind::Binary,
            binary_path: file.display().to_string(),
            params: vec![],
            headers: vec![],
            ..Default::default()
        };
        let _ = send_to(spec);
        let seen = server.join().unwrap();
        assert!(seen.starts_with("PUT /blob HTTP/1.1"), "{seen}");
        assert!(
            seen.to_ascii_lowercase().contains("content-type: application/xml"),
            "content type not guessed:\n{seen}"
        );
        assert!(seen.contains("<note>hi</note>"), "bytes missing:\n{seen}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_upload_file_fails_before_sending() {
        let spec = RequestSpec {
            url: "http://127.0.0.1:1/x".to_owned(),
            body_kind: BodyKind::Binary,
            binary_path: "no/such/file.bin".to_owned(),
            params: vec![],
            headers: vec![],
            ..Default::default()
        };
        let err = run(
            spec,
            SendOpts {
                timeout_secs: 5,
                insecure_tls: false,
            },
        )
        .unwrap_err();
        assert!(err.contains("no/such/file.bin"), "{err}");
    }

    #[test]
    fn explicit_content_type_beats_the_body_kind_default() {
        let (port, server) = one_shot_server();
        let spec = RequestSpec {
            method: Method::Post,
            url: format!("localhost:{port}/x"),
            body_kind: BodyKind::Json,
            body: "{}".to_owned(),
            headers: vec![KeyVal {
                on: true,
                key: "Content-Type".into(),
                value: "application/vnd.api+json".into(),
            }],
            params: vec![],
            ..Default::default()
        };
        let _ = send_to(spec);
        let seen = server.join().unwrap().to_ascii_lowercase();
        assert!(seen.contains("application/vnd.api+json"), "{seen}");
        assert_eq!(
            seen.matches("content-type:").count(),
            1,
            "duplicate content-type sent:\n{seen}"
        );
    }


    #[test]
    fn shape_from_content_type_then_sniffing() {
        use crate::net::{detect_shape, Shape};
        assert_eq!(detect_shape("application/json", b"{}"), Shape::Json);
        assert_eq!(
            detect_shape("application/problem+json", b"{}"),
            Shape::Json
        );
        assert_eq!(detect_shape("text/html; charset=utf-8", b"<p>"), Shape::Html);
        assert_eq!(detect_shape("image/png", b""), Shape::Image);
        assert_eq!(detect_shape("application/pdf", b"%PDF"), Shape::Binary);
        // no content type: sniff instead
        assert_eq!(detect_shape("", br#"  {"a":1}"#), Shape::Json);
        assert_eq!(detect_shape("", b"<!doctype html><p>"), Shape::Html);
        assert_eq!(detect_shape("", b"<rss><item/></rss>"), Shape::Xml);
        assert_eq!(detect_shape("", b"\x89PNG\r\n"), Shape::Image);
        assert_eq!(detect_shape("", b"plain words"), Shape::Text);
        assert_eq!(detect_shape("", &[0xff, 0xfe, 0x00, 0x01]), Shape::Binary);
        // octet-stream is not trusted over a real sniff
        assert_eq!(
            detect_shape("application/octet-stream", br#"{"a":1}"#),
            Shape::Json
        );
    }

    #[test]
    fn set_cookie_headers_are_split_up() {
        let resp = ResponseData {
            status: 200,
            status_text: "OK".into(),
            elapsed_ms: 1,
            size: 0,
            headers: vec![
                (
                    "set-cookie".into(),
                    "session=abc123; Path=/; HttpOnly; SameSite=Lax".into(),
                ),
                ("set-cookie".into(), "theme=dark".into()),
                ("content-type".into(), "text/plain".into()),
            ],
            body: String::new(),
            pretty: None,
            final_url: String::new(),
            content_type: "text/plain".into(),
            shape: Shape::Text,
            bytes: Vec::new(),
        };
        let cookies = resp.cookies();
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies[0].0, "session");
        assert_eq!(cookies[0].1, "abc123");
        assert_eq!(cookies[0].2, "Path=/; HttpOnly; SameSite=Lax");
        assert_eq!(cookies[1], ("theme".into(), "dark".into(), String::new()));
    }

    #[test]
    fn refused_connection_says_what_to_check() {
        // bind then drop: nothing is listening on that port any more
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let spec = RequestSpec {
            url: format!("localhost:{port}/"),
            params: vec![],
            headers: vec![],
            ..Default::default()
        };
        let err = run(
            spec,
            SendOpts {
                timeout_secs: 5,
                insecure_tls: false,
            },
        )
        .unwrap_err();
        assert!(err.contains("dev server"), "unhelpful error: {err}");
    }
}
