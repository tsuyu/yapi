//! cURL in both directions, over the same `RequestSpec` everything else uses.
//!
//! Import parses a command line into the model; export writes the model back
//! out. Neither shells out to curl.

use crate::model::{
    ApiKeyIn, Auth, AuthKind, BodyKind, Extract, FormPart, KeyVal, Method, RequestSpec,
};

// ---------------------------------------------------------------- import ----

/// Does this text look like a curl command rather than a plain url? Used to
/// catch a curl command pasted into the url bar by mistake.
pub fn looks_like_curl(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("curl ")
        || t.starts_with("curl\t")
        || t == "curl"
        // a url never contains these; a curl command usually does
        || t.contains(" -H ")
        || t.contains(" --header ")
        || t.contains(" -d ")
        || t.contains(" --data")
        || t.contains(" -F ")
}

/// Parse a `curl ...` command line into a request.
pub fn parse(input: &str) -> Result<RequestSpec, String> {
    let tokens = tokenize(input)?;
    if tokens.is_empty() {
        return Err("nothing to parse".to_owned());
    }

    let mut spec = RequestSpec {
        name: "imported".to_owned(),
        params: Vec::new(),
        path_params: Vec::new(),
        headers: Vec::new(),
        cookies: Vec::new(),
        form_parts: Vec::new(),
        extract: Vec::new(),
        auth: Auth {
            kind: AuthKind::None,
            ..Default::default()
        },
        ..Default::default()
    };
    spec.transport.follow_redirects = false;
    spec.transport.compressed = false;

    let mut url = String::new();
    let mut method: Option<Method> = None;
    let mut data: Vec<String> = Vec::new();
    let mut data_is_binary_file: Option<String> = None;
    let mut data_urlencoded: Vec<KeyVal> = Vec::new();
    let mut as_get = false;
    let mut ignored: Vec<String> = Vec::new();
    let mut insecure = false;
    let mut timeout: Option<u64> = None;

    let mut i = 0;
    while i < tokens.len() {
        let token = tokens[i].clone();
        i += 1;

        // the leading "curl", and shell noise people paste with it
        if i == 1 && (token == "curl" || token.ends_with("/curl") || token.ends_with("curl.exe")) {
            continue;
        }

        let next = |flag: &str, i: &mut usize| -> Result<String, String> {
            if *i < tokens.len() {
                let v = tokens[*i].clone();
                *i += 1;
                Ok(v)
            } else {
                Err(format!("{flag} needs a value"))
            }
        };

        // --flag=value is the same as --flag value
        let (name, inline) = match token.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n.to_owned(), Some(v.to_owned())),
            _ => (token.clone(), None),
        };
        let take = |flag: &str, i: &mut usize| -> Result<String, String> {
            match &inline {
                Some(v) => Ok(v.clone()),
                None => next(flag, i),
            }
        };

        match name.as_str() {
            "-X" | "--request" => {
                let raw = take("-X", &mut i)?;
                method = Some(parse_method(&raw)?);
            }
            "-H" | "--header" => {
                let raw = take("-H", &mut i)?;
                match raw.split_once(':') {
                    Some((k, v)) => push_header(&mut spec, k.trim(), v.trim()),
                    // "Header;" is curl's way of sending an empty header
                    None => push_header(&mut spec, raw.trim_end_matches(';').trim(), ""),
                }
            }
            "-d" | "--data" | "--data-raw" | "--data-ascii" => {
                let raw = take("-d", &mut i)?;
                data.push(read_at_file(&raw, name != "--data-raw")?);
            }
            "--data-binary" => {
                let raw = take("--data-binary", &mut i)?;
                match raw.strip_prefix('@') {
                    Some(path) => data_is_binary_file = Some(path.to_owned()),
                    None => data.push(raw),
                }
            }
            "--data-urlencode" => {
                let raw = take("--data-urlencode", &mut i)?;
                let (k, v) = raw.split_once('=').unwrap_or(("", raw.as_str()));
                data_urlencoded.push(KeyVal {
                    on: true,
                    key: k.to_owned(),
                    value: v.to_owned(),
                });
            }
            "--json" => {
                let raw = take("--json", &mut i)?;
                data.push(read_at_file(&raw, true)?);
                spec.body_kind = BodyKind::Json;
                push_header(&mut spec, "Content-Type", "application/json");
                push_header(&mut spec, "Accept", "application/json");
            }
            "-F" | "--form" | "--form-string" => {
                let raw = take("-F", &mut i)?;
                spec.form_parts.push(parse_form_part(&raw));
                spec.body_kind = BodyKind::Multipart;
            }
            "-b" | "--cookie" => {
                let raw = take("-b", &mut i)?;
                if raw.contains('=') {
                    spec.cookies.extend(crate::model::parse_query(
                        &raw.split(';')
                            .map(str::trim)
                            .collect::<Vec<_>>()
                            .join("&"),
                    ));
                } else {
                    ignored.push(format!("cookie jar file {raw}"));
                }
            }
            "-u" | "--user" => {
                let raw = take("-u", &mut i)?;
                let (user, pass) = raw.split_once(':').unwrap_or((raw.as_str(), ""));
                spec.auth.kind = AuthKind::Basic;
                spec.auth.username = user.to_owned();
                spec.auth.password = pass.to_owned();
            }
            "--oauth2-bearer" => {
                let raw = take("--oauth2-bearer", &mut i)?;
                spec.auth.kind = AuthKind::Bearer;
                spec.auth.token = raw;
            }
            "-A" | "--user-agent" => {
                let raw = take("-A", &mut i)?;
                push_header(&mut spec, "User-Agent", &raw);
            }
            "-e" | "--referer" => {
                let raw = take("-e", &mut i)?;
                push_header(&mut spec, "Referer", &raw);
            }
            "-T" | "--upload-file" => {
                let raw = take("-T", &mut i)?;
                spec.body_kind = BodyKind::Binary;
                spec.binary_path = raw;
                method.get_or_insert(Method::Put);
            }
            "-G" | "--get" => as_get = true,
            "-I" | "--head" => method = Some(Method::Head),
            "-L" | "--location" => spec.transport.follow_redirects = true,
            "--max-redirs" => {
                let raw = take("--max-redirs", &mut i)?;
                spec.transport.max_redirects = raw.parse().unwrap_or(10);
            }
            "--compressed" => spec.transport.compressed = true,
            "-k" | "--insecure" => insecure = true,
            "-x" | "--proxy" => spec.transport.proxy = take("-x", &mut i)?,
            "--cacert" => spec.transport.ca_cert = take("--cacert", &mut i)?,
            "-E" | "--cert" => spec.transport.client_cert = take("-E", &mut i)?,
            "--key" => spec.transport.client_key = take("--key", &mut i)?,
            "-m" | "--max-time" | "--connect-timeout" => {
                let raw = take("-m", &mut i)?;
                timeout = raw.parse::<f64>().ok().map(|secs| secs.ceil() as u64);
            }
            "--url" => url = take("--url", &mut i)?,
            // noise that does not change the request we would send
            "-s" | "--silent" | "-v" | "--verbose" | "-i" | "--include" | "-f" | "--fail"
            | "-#" | "--progress-bar" | "-S" | "--show-error" | "--no-buffer" | "-N" => {}
            "-o" | "--output" | "-w" | "--write-out" | "--retry" | "-c" | "--cookie-jar" => {
                let raw = take(&name, &mut i)?;
                ignored.push(format!("{name} {raw}"));
            }
            other if other.starts_with('-') && other.len() > 1 => {
                ignored.push(other.to_owned());
            }
            bare => {
                if url.is_empty() {
                    url = bare.to_owned();
                } else {
                    ignored.push(bare.to_owned());
                }
            }
        }
    }

    if url.is_empty() {
        return Err("no url in that command".to_owned());
    }

    // body assembly
    let joined = data.join("&");
    if !data_urlencoded.is_empty() {
        let encoded = crate::model::urlencode_pairs(&data_urlencoded);
        if as_get {
            spec.params.extend(data_urlencoded);
        } else {
            spec.body = if joined.is_empty() {
                encoded
            } else {
                format!("{joined}&{encoded}")
            };
            spec.body_kind = BodyKind::Form;
        }
    }
    if let Some(path) = data_is_binary_file {
        spec.body_kind = BodyKind::Binary;
        spec.binary_path = path;
    } else if !joined.is_empty() {
        if as_get {
            spec.params.extend(crate::model::parse_query(&joined));
        } else {
            spec.body = joined;
            if spec.body_kind == BodyKind::None {
                spec.body_kind = body_kind_from_headers(&spec, &spec.body);
            }
        }
    }

    // an explicit -X wins; otherwise a body implies POST
    spec.method = method.unwrap_or({
        if spec.body_kind == BodyKind::None {
            Method::Get
        } else {
            Method::Post
        }
    });

    spec.url = url;
    spec.pull_params_from_url();
    spec.params.retain(|p| !p.key.trim().is_empty());
    adopt_auth_header(&mut spec);
    spec.sync_path_params();

    // leave the editor tables with a blank row to type into, as elsewhere
    spec.params.push(KeyVal::default());
    spec.headers.push(KeyVal::default());
    spec.cookies.push(KeyVal::default());
    spec.extract.push(Extract::default());
    if spec.body_kind == BodyKind::Multipart {
        spec.form_parts.push(FormPart::default());
    }

    spec.name = name_from_url(&spec.url, spec.method);
    let _ = (insecure, timeout, ignored);
    Ok(spec)
}

/// What the import could not represent, for telling the user.
pub struct Imported {
    pub spec: RequestSpec,
    /// `-k` and `-m` are app-wide settings here, not per request.
    pub insecure_tls: bool,
    pub timeout_secs: Option<u64>,
    pub ignored: Vec<String>,
}

/// Parse, and also report the flags that map onto app settings or nothing.
pub fn parse_full(input: &str) -> Result<Imported, String> {
    let spec = parse(input)?;
    let tokens = tokenize(input)?;
    let insecure_tls = tokens.iter().any(|t| t == "-k" || t == "--insecure");
    let timeout_secs = tokens
        .iter()
        .position(|t| t == "-m" || t == "--max-time")
        .and_then(|i| tokens.get(i + 1))
        .and_then(|v| v.parse::<f64>().ok())
        .map(|secs| secs.ceil() as u64);
    let ignored = tokens
        .iter()
        .filter(|t| {
            matches!(
                t.as_str(),
                "-o" | "--output" | "-w" | "--write-out" | "-c" | "--cookie-jar" | "--retry"
            )
        })
        .cloned()
        .collect();
    Ok(Imported {
        spec,
        insecure_tls,
        timeout_secs,
        ignored,
    })
}

fn parse_method(raw: &str) -> Result<Method, String> {
    Method::ALL
        .into_iter()
        .find(|m| m.as_str().eq_ignore_ascii_case(raw.trim()))
        .ok_or_else(|| format!("unsupported method {raw:?}"))
}

fn push_header(spec: &mut RequestSpec, key: &str, value: &str) {
    if key.is_empty() {
        return;
    }
    if key.eq_ignore_ascii_case("cookie") {
        spec.cookies.extend(crate::model::parse_query(
            &value.split(';').map(str::trim).collect::<Vec<_>>().join("&"),
        ));
        return;
    }
    spec.headers.push(KeyVal {
        on: true,
        key: key.to_owned(),
        value: value.to_owned(),
    });
}

/// `@file` means "read the file" for -d, but not for --data-raw.
fn read_at_file(raw: &str, expand: bool) -> Result<String, String> {
    match raw.strip_prefix('@') {
        Some(path) if expand => {
            std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
        }
        _ => Ok(raw.to_owned()),
    }
}

fn parse_form_part(raw: &str) -> FormPart {
    let (key, rest) = raw.split_once('=').unwrap_or((raw, ""));
    let mut part = FormPart {
        on: true,
        key: key.to_owned(),
        ..Default::default()
    };
    // value can carry ;type=... and ;filename=...
    let mut pieces = rest.split(';');
    let head = pieces.next().unwrap_or("");
    for extra in pieces {
        if let Some(t) = extra.trim().strip_prefix("type=") {
            part.content_type = t.to_owned();
        }
    }
    match head.strip_prefix('@').or_else(|| head.strip_prefix('<')) {
        Some(path) => part.file = path.to_owned(),
        None => part.value = head.to_owned(),
    }
    part
}

fn body_kind_from_headers(spec: &RequestSpec, body: &str) -> BodyKind {
    let ct = spec
        .headers
        .iter()
        .find(|h| h.key.eq_ignore_ascii_case("content-type"))
        .map(|h| h.value.to_ascii_lowercase())
        .unwrap_or_default();
    if ct.contains("json") {
        BodyKind::Json
    } else if ct.contains("xml") {
        BodyKind::Xml
    } else if ct.contains("x-www-form-urlencoded") {
        BodyKind::Form
    } else if !ct.is_empty() {
        BodyKind::Text
    } else if body.trim_start().starts_with('{') || body.trim_start().starts_with('[') {
        BodyKind::Json
    } else if body.contains('=') && !body.contains(' ') {
        BodyKind::Form
    } else {
        BodyKind::Text
    }
}

/// Turn an `Authorization:` header into the matching auth mode, so the auth tab
/// shows a bearer token rather than a raw header row.
fn adopt_auth_header(spec: &mut RequestSpec) {
    let Some(pos) = spec
        .headers
        .iter()
        .position(|h| h.key.eq_ignore_ascii_case("authorization"))
    else {
        return;
    };
    if spec.auth.kind != AuthKind::None {
        return;
    }
    let value = spec.headers[pos].value.trim().to_owned();
    let lower = value.to_ascii_lowercase();
    if let Some(token) = lower.strip_prefix("bearer ") {
        let token = value[value.len() - token.len()..].to_owned();
        // a jwt is still a bearer token; the auth tab decodes it either way
        spec.auth.kind = AuthKind::Bearer;
        spec.auth.token = token;
        spec.headers.remove(pos);
    } else if lower.starts_with("basic ") {
        let encoded = value[6..].trim();
        if let Ok(bytes) = crate::model::base64_url_decode(encoded) {
            if let Ok(text) = String::from_utf8(bytes) {
                let (user, pass) = text.split_once(':').unwrap_or((text.as_str(), ""));
                spec.auth.kind = AuthKind::Basic;
                spec.auth.username = user.to_owned();
                spec.auth.password = pass.to_owned();
                spec.headers.remove(pos);
            }
        }
    }
}

fn name_from_url(url: &str, method: Method) -> String {
    let path = crate::model::normalize_url(url);
    let path = path
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(&path);
    let tail = path.split(['?', '#']).next().unwrap_or(path);
    format!("{} {tail}", method.as_str())
}

/// Split a command line the way a posix shell would: quotes, backslash escapes,
/// and `\` or `^` line continuations.
fn tokenize(input: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.peek() {
                // line continuation
                Some('\n') => {
                    chars.next();
                }
                Some('\r') => {
                    chars.next();
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                }
                Some(&escaped) => {
                    chars.next();
                    current.push(escaped);
                    has_token = true;
                }
                None => {}
            },
            '^' if chars.peek() == Some(&'\n') => {
                // windows cmd continuation
                chars.next();
            }
            '\'' => {
                has_token = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    current.push(c);
                }
            }
            '"' => {
                has_token = true;
                let mut closed = false;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => {
                            closed = true;
                            break;
                        }
                        '\\' => match chars.next() {
                            Some('\n') => {}
                            Some(escaped) => current.push(escaped),
                            None => {}
                        },
                        other => current.push(other),
                    }
                }
                if !closed {
                    return Err("unterminated double quote".to_owned());
                }
            }
            c if c.is_whitespace() => {
                if has_token {
                    tokens.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            other => {
                current.push(other);
                has_token = true;
            }
        }
    }
    if has_token {
        tokens.push(current);
    }
    Ok(tokens)
}

// ---------------------------------------------------------------- export ----

/// Write a request out as a curl command line. `resolved` decides whether
/// `{{variables}}` are substituted or left visible.
pub fn generate(spec: &RequestSpec, insecure_tls: bool, timeout_secs: Option<u64>) -> String {
    let mut lines: Vec<String> = Vec::new();
    let method = spec.method;

    // the url is always quoted: a bare `&` in a query would fork the shell
    let url = quote_always(&spec.full_url());
    let head = if method == Method::Get {
        format!("curl {url}")
    } else {
        format!("curl -X {} {url}", method.as_str())
    };
    lines.push(head);

    if let Some((k, v)) = spec.auth.header() {
        lines.push(format!("-H {}", quote(&format!("{k}: {v}"))));
    }
    for h in spec.headers.iter().filter(|h| h.active()) {
        lines.push(format!(
            "-H {}",
            quote(&format!("{}: {}", h.key.trim(), h.value))
        ));
    }
    if let Some(ct) = spec.body_kind.content_type() {
        let already = spec
            .headers
            .iter()
            .any(|h| h.active() && h.key.trim().eq_ignore_ascii_case("content-type"));
        if !already {
            lines.push(format!("-H {}", quote(&format!("Content-Type: {ct}"))));
        }
    }
    if let Some(cookies) = spec.cookie_header() {
        lines.push(format!("-b {}", quote(&cookies)));
    }

    match spec.body_kind {
        BodyKind::None => {}
        BodyKind::Multipart => {
            for part in spec.form_parts.iter().filter(|p| p.active()) {
                let value = if part.is_file() {
                    let mut v = format!("{}=@{}", part.key.trim(), part.file.trim());
                    if !part.content_type.trim().is_empty() {
                        v.push_str(&format!(";type={}", part.content_type.trim()));
                    }
                    v
                } else {
                    format!("{}={}", part.key.trim(), part.value)
                };
                lines.push(format!("-F {}", quote(&value)));
            }
        }
        BodyKind::Binary => {
            if !spec.binary_path.trim().is_empty() {
                lines.push(format!(
                    "--data-binary {}",
                    quote(&format!("@{}", spec.binary_path.trim()))
                ));
            }
        }
        BodyKind::Form => {
            let pairs = crate::model::parse_query(&spec.body.replace(['\n', '\r'], "&"));
            lines.push(format!(
                "--data-raw {}",
                quote(&crate::model::urlencode_pairs(&pairs))
            ));
        }
        _ => {
            if !spec.body.is_empty() {
                lines.push(format!("--data-raw {}", quote(&spec.body)));
            }
        }
    }

    let t = &spec.transport;
    if t.follow_redirects {
        lines.push("-L".to_owned());
        if t.max_redirects != 10 {
            lines.push(format!("--max-redirs {}", t.max_redirects));
        }
    }
    if t.compressed {
        lines.push("--compressed".to_owned());
    }
    if !t.proxy.trim().is_empty() {
        lines.push(format!("-x {}", quote(t.proxy.trim())));
    }
    if !t.ca_cert.trim().is_empty() {
        lines.push(format!("--cacert {}", quote(t.ca_cert.trim())));
    }
    if !t.client_cert.trim().is_empty() {
        lines.push(format!("-E {}", quote(t.client_cert.trim())));
    }
    if !t.client_key.trim().is_empty() {
        lines.push(format!("--key {}", quote(t.client_key.trim())));
    }
    if insecure_tls {
        lines.push("-k".to_owned());
    }
    if let Some(secs) = timeout_secs {
        lines.push(format!("-m {secs}"));
    }
    if spec.auth.kind == AuthKind::ApiKey && spec.auth.api_key_in == ApiKeyIn::Query {
        // already folded into full_url(), nothing more to add
    }

    lines.join(" \\\n  ")
}

fn quote_always(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Single-quote for a posix shell, the way curl's own copy-as does it.
fn quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_alphanumeric() || "-_./:=@%+,".contains(c))
    {
        return value.to_owned();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Transport;

    fn header<'a>(spec: &'a RequestSpec, name: &str) -> Option<&'a str> {
        spec.headers
            .iter()
            .find(|h| h.key.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }

    #[test]
    fn imports_the_readme_example() {
        let spec = parse(
            r#"curl -X POST "https://api.example.com/users" \
  -H "Authorization: Bearer abc123" \
  -H "Content-Type: application/json" \
  -d '{"name":"John","email":"john@example.com"}'"#,
        )
        .unwrap();

        assert_eq!(spec.method, Method::Post);
        assert_eq!(spec.url, "https://api.example.com/users");
        assert_eq!(header(&spec, "content-type"), Some("application/json"));
        assert_eq!(spec.body_kind, BodyKind::Json);
        assert_eq!(spec.body, r#"{"name":"John","email":"john@example.com"}"#);
        // the Authorization header became a bearer credential
        assert_eq!(spec.auth.kind, AuthKind::Bearer);
        assert_eq!(spec.auth.token, "abc123");
        assert!(header(&spec, "authorization").is_none());
    }

    #[test]
    fn query_params_come_out_of_the_url() {
        let spec = parse("curl 'https://x.dev/search?q=hello+world&page=2'").unwrap();
        assert_eq!(spec.method, Method::Get);
        assert_eq!(spec.url, "https://x.dev/search");
        let params: Vec<_> = spec
            .params
            .iter()
            .filter(|p| p.active())
            .map(|p| (p.key.as_str(), p.value.as_str()))
            .collect();
        assert_eq!(params, vec![("q", "hello world"), ("page", "2")]);
    }

    #[test]
    fn basic_auth_from_both_spellings() {
        let from_flag = parse("curl -u alice:s3cret https://x.dev").unwrap();
        assert_eq!(from_flag.auth.kind, AuthKind::Basic);
        assert_eq!(from_flag.auth.username, "alice");
        assert_eq!(from_flag.auth.password, "s3cret");

        let encoded = crate::model::base64(b"alice:s3cret");
        let from_header = parse(&format!(
            "curl -H 'Authorization: Basic {encoded}' https://x.dev"
        ))
        .unwrap();
        assert_eq!(from_header.auth.kind, AuthKind::Basic);
        assert_eq!(from_header.auth.username, "alice");
        assert_eq!(from_header.auth.password, "s3cret");
    }

    #[test]
    fn cookies_from_flag_and_header() {
        let spec = parse("curl -b 'a=1; b=2' -H 'Cookie: c=3' https://x.dev").unwrap();
        let cookies: Vec<_> = spec
            .cookies
            .iter()
            .filter(|c| c.active())
            .map(|c| (c.key.as_str(), c.value.as_str()))
            .collect();
        assert_eq!(cookies, vec![("a", "1"), ("b", "2"), ("c", "3")]);
        assert!(header(&spec, "cookie").is_none());
    }

    #[test]
    fn multipart_and_file_uploads() {
        let spec = parse(
            "curl -F 'title=a caption' -F 'file=@/tmp/x.png;type=image/png' https://x.dev/upload",
        )
        .unwrap();
        assert_eq!(spec.method, Method::Post);
        assert_eq!(spec.body_kind, BodyKind::Multipart);
        let parts: Vec<_> = spec.form_parts.iter().filter(|p| p.active()).collect();
        assert_eq!(parts[0].key, "title");
        assert_eq!(parts[0].value, "a caption");
        assert_eq!(parts[1].file, "/tmp/x.png");
        assert_eq!(parts[1].content_type, "image/png");
    }

    #[test]
    fn data_binary_file_becomes_a_binary_body() {
        let spec = parse("curl --data-binary @payload.bin -X PUT https://x.dev/blob").unwrap();
        assert_eq!(spec.body_kind, BodyKind::Binary);
        assert_eq!(spec.binary_path, "payload.bin");
        assert_eq!(spec.method, Method::Put);
    }

    #[test]
    fn get_with_data_puts_it_in_the_query() {
        let spec = parse("curl -G -d q=rust -d lang=en https://x.dev/search").unwrap();
        assert_eq!(spec.method, Method::Get);
        let params: Vec<_> = spec
            .params
            .iter()
            .filter(|p| p.active())
            .map(|p| (p.key.as_str(), p.value.as_str()))
            .collect();
        assert_eq!(params, vec![("q", "rust"), ("lang", "en")]);
        assert_eq!(spec.body, "");
    }

    #[test]
    fn transport_flags_are_captured() {
        let spec = parse(
            "curl -L --max-redirs 3 --compressed -x http://proxy:8080 \
             --cacert /etc/ca.pem -E client.pem --key client.key https://x.dev",
        )
        .unwrap();
        assert!(spec.transport.follow_redirects);
        assert_eq!(spec.transport.max_redirects, 3);
        assert!(spec.transport.compressed);
        assert_eq!(spec.transport.proxy, "http://proxy:8080");
        assert_eq!(spec.transport.ca_cert, "/etc/ca.pem");
        assert_eq!(spec.transport.client_cert, "client.pem");
        assert_eq!(spec.transport.client_key, "client.key");
    }

    #[test]
    fn app_level_flags_are_reported_separately() {
        let out = parse_full("curl -k -m 5 -o out.json https://x.dev").unwrap();
        assert!(out.insecure_tls);
        assert_eq!(out.timeout_secs, Some(5));
        assert!(out.ignored.iter().any(|f| f == "-o"));
    }

    #[test]
    fn tokenizer_handles_quotes_and_continuations() {
        let tokens = tokenize("curl 'a b' \"c\\\"d\" e\\ f \\\n --flag=value").unwrap();
        assert_eq!(tokens, vec!["curl", "a b", "c\"d", "e f", "--flag=value"]);
        assert!(tokenize("curl \"unterminated").is_err());
    }

    #[test]
    fn flag_with_equals_is_the_same_as_a_space() {
        let a = parse("curl --request=DELETE https://x.dev/1").unwrap();
        let b = parse("curl --request DELETE https://x.dev/1").unwrap();
        assert_eq!(a.method, Method::Delete);
        assert_eq!(a.method, b.method);
    }

    #[test]
    fn generates_the_readme_example() {
        let spec = RequestSpec {
            method: Method::Post,
            url: "https://api.example.com/users".to_owned(),
            params: vec![],
            headers: vec![KeyVal {
                on: true,
                key: "Content-Type".into(),
                value: "application/json".into(),
            }],
            cookies: vec![],
            body_kind: BodyKind::Json,
            body: r#"{"name":"John"}"#.to_owned(),
            auth: Auth {
                kind: AuthKind::Bearer,
                token: "{{token}}".to_owned(),
                ..Default::default()
            },
            transport: Transport {
                follow_redirects: false,
                compressed: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let out = generate(&spec, false, None);
        assert_eq!(
            out,
            "curl -X POST 'https://api.example.com/users' \\\n  \
             -H 'Authorization: Bearer {{token}}' \\\n  \
             -H 'Content-Type: application/json' \\\n  \
             --data-raw '{\"name\":\"John\"}'"
        );
    }

    #[test]
    fn generated_commands_parse_back_to_the_same_request() {
        let original = RequestSpec {
            method: Method::Put,
            url: "https://api.example.com/things".to_owned(),
            params: vec![KeyVal {
                on: true,
                key: "page".into(),
                value: "2".into(),
            }],
            headers: vec![KeyVal {
                on: true,
                key: "X-Trace".into(),
                value: "it's here".into(),
            }],
            cookies: vec![KeyVal {
                on: true,
                key: "sid".into(),
                value: "abc".into(),
            }],
            body_kind: BodyKind::Json,
            body: r#"{"a":1}"#.to_owned(),
            auth: Auth {
                kind: AuthKind::Bearer,
                token: "tok".to_owned(),
                ..Default::default()
            },
            transport: Transport {
                follow_redirects: true,
                max_redirects: 3,
                compressed: true,
                proxy: "http://proxy:8080".to_owned(),
                ..Default::default()
            },
            ..Default::default()
        };

        let command = generate(&original, false, None);
        let back = parse(&command).unwrap();

        assert_eq!(back.method, original.method);
        assert_eq!(back.url, original.url);
        assert_eq!(back.body, original.body);
        assert_eq!(back.body_kind, original.body_kind);
        assert_eq!(back.auth.kind, AuthKind::Bearer);
        assert_eq!(back.auth.token, "tok");
        assert_eq!(header(&back, "x-trace"), Some("it's here"));
        assert_eq!(
            back.params
                .iter()
                .filter(|p| p.active())
                .map(|p| (p.key.clone(), p.value.clone()))
                .collect::<Vec<_>>(),
            vec![("page".to_owned(), "2".to_owned())]
        );
        assert_eq!(
            back.cookies
                .iter()
                .filter(|c| c.active())
                .map(|c| (c.key.clone(), c.value.clone()))
                .collect::<Vec<_>>(),
            vec![("sid".to_owned(), "abc".to_owned())]
        );
        assert_eq!(back.transport.proxy, "http://proxy:8080");
        assert_eq!(back.transport.max_redirects, 3);
        assert!(back.transport.follow_redirects && back.transport.compressed);
    }

    #[test]
    fn multipart_round_trips() {
        let spec = parse("curl -F 'a=1' -F 'f=@x.png;type=image/png' https://x.dev/u").unwrap();
        let back = parse(&generate(&spec, false, None)).unwrap();
        let parts: Vec<_> = back.form_parts.iter().filter(|p| p.active()).collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1].file, "x.png");
        assert_eq!(parts[1].content_type, "image/png");
    }

    #[test]
    fn quoting_survives_awkward_values() {
        assert_eq!(quote("plain"), "plain");
        assert_eq!(quote("has space"), "'has space'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote(""), "''");
        // a value with an embedded quote survives the round trip
        let tokens = tokenize(&format!("curl -d {}", quote("it's a {\"json\":1}"))).unwrap();
        assert_eq!(tokens[2], "it's a {\"json\":1}");
    }

    #[test]
    fn recognises_a_curl_command_vs_a_url() {
        assert!(looks_like_curl("curl https://x.dev"));
        assert!(looks_like_curl("  curl -X POST https://x.dev"));
        assert!(looks_like_curl("wget-ish -H 'a: b' https://x.dev"));
        assert!(looks_like_curl("curl"));
        // plain urls are not curl
        assert!(!looks_like_curl("https://x.dev/users?q=1"));
        assert!(!looks_like_curl("localhost:3000/api"));
        assert!(!looks_like_curl("flounder.epizy.com"));
    }

    #[test]
    fn parses_even_when_newlines_were_stripped() {
        // egui's single-line field drops newlines, mashing a pasted multi-line
        // command; the leading-backslash continuations survive as tokens
        let mashed = "curl -H \"authority: flounder.epizy.com\"      -H \"Cookie: __test=abc\"      http://flounder.epizy.com";
        let spec = parse(mashed).unwrap();
        assert_eq!(spec.url, "http://flounder.epizy.com");
        assert!(!spec.url.contains("curl"), "url was mangled: {}", spec.url);
    }

    #[test]
    fn rejects_input_that_is_not_a_request() {
        assert!(parse("").is_err());
        assert!(parse("curl -X POST").unwrap_err().contains("no url"));
        assert!(parse("curl -X FLY https://x.dev")
            .unwrap_err()
            .contains("unsupported method"));
        assert!(parse("curl -H").unwrap_err().contains("needs a value"));
    }
}


