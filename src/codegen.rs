//! Write a request out as code in someone else's language. Same idea as
//! `curl::generate` - read a `RequestSpec`, emit text - so what you send from
//! here is what you paste into a codebase.
//!
//! These are one-way. Only curl parses back in.

use crate::model::{BodyKind, RequestSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Curl,
    Fetch,
    Python,
    Httpie,
}

impl Target {
    pub const ALL: [Target; 4] = [Target::Curl, Target::Fetch, Target::Python, Target::Httpie];

    pub fn as_str(self) -> &'static str {
        match self {
            Target::Curl => "curl",
            Target::Fetch => "javascript fetch",
            Target::Python => "python requests",
            Target::Httpie => "httpie",
        }
    }
}

/// Render `spec` for `target`. `insecure_tls` and `timeout_secs` are app-wide
/// settings, passed in because they are not part of the request.
pub fn generate(
    target: Target,
    spec: &RequestSpec,
    insecure_tls: bool,
    timeout_secs: Option<u64>,
) -> String {
    match target {
        Target::Curl => crate::curl::generate(spec, insecure_tls, timeout_secs),
        Target::Fetch => fetch(spec),
        Target::Python => python(spec, insecure_tls, timeout_secs),
        Target::Httpie => httpie(spec, insecure_tls),
    }
}

/// Every header this request sends, auth and body content type folded in, in
/// the order the wire sees them. An explicit row always wins, matching `net`.
fn headers_of(spec: &RequestSpec) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let explicit = |name: &str| {
        spec.headers
            .iter()
            .any(|h| h.active() && h.key.trim().eq_ignore_ascii_case(name))
    };

    if let Some((k, v)) = spec.auth.header() {
        if !explicit(&k) {
            out.push((k, v));
        }
    }
    for h in spec.headers.iter().filter(|h| h.active()) {
        out.push((h.key.trim().to_owned(), h.value.clone()));
    }
    if let Some(ct) = spec.body_kind.content_type() {
        if !explicit("content-type") {
            out.push(("Content-Type".to_owned(), ct.to_owned()));
        }
    }
    if let Some(cookies) = spec.cookie_header() {
        if !explicit("cookie") {
            out.push(("Cookie".to_owned(), cookies));
        }
    }
    out
}

/// A note for the things a target cannot express, appended as a comment rather
/// than dropped silently.
fn unsupported(spec: &RequestSpec) -> Vec<String> {
    let mut out = Vec::new();
    if spec.body_kind == BodyKind::Binary && !spec.binary_path.trim().is_empty() {
        out.push(format!("binary body from {}", spec.binary_path.trim()));
    }
    let files: Vec<&str> = spec
        .form_parts
        .iter()
        .filter(|p| p.active() && p.is_file())
        .map(|p| p.file.trim())
        .collect();
    if !files.is_empty() {
        out.push(format!("file upload(s): {}", files.join(", ")));
    }
    let t = &spec.transport;
    if !t.ca_cert.trim().is_empty() {
        out.push(format!("extra ca cert {}", t.ca_cert.trim()));
    }
    if !t.client_cert.trim().is_empty() {
        out.push(format!("client cert {}", t.client_cert.trim()));
    }
    out
}

fn js_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Python string literal. Multi-line bodies get triple quotes so json stays
/// readable in the paste.
fn py_string(text: &str) -> String {
    if text.contains('\n') && !text.contains("\"\"\"") {
        return format!("\"\"\"{}\"\"\"", text.replace('\\', "\\\\"));
    }
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn fetch(spec: &RequestSpec) -> String {
    let mut lines = Vec::new();
    for note in unsupported(spec) {
        lines.push(format!("// not expressed below: {note}"));
    }

    lines.push(format!("const res = await fetch({}, {{", js_string(&spec.full_url())));
    lines.push(format!("  method: {},", js_string(spec.method.as_str())));

    let headers = headers_of(spec);
    if headers.is_empty() {
        lines.push("  headers: {},".to_owned());
    } else {
        lines.push("  headers: {".to_owned());
        for (k, v) in &headers {
            lines.push(format!("    {}: {},", js_string(k), js_string(v)));
        }
        lines.push("  },".to_owned());
    }

    match spec.body_kind {
        BodyKind::None => {}
        BodyKind::Multipart => {
            // built above the call, referenced here
            lines.push("  body: form,".to_owned());
        }
        BodyKind::Binary => lines.push("  // body: <file bytes>".to_owned()),
        BodyKind::GraphQl => lines.push(format!(
            "  body: {},",
            js_string(&crate::model::graphql_body(&spec.body, &spec.graphql_vars))
        )),
        BodyKind::Form => {
            let pairs = crate::model::parse_query(&spec.body.replace(['\n', '\r'], "&"));
            lines.push(format!(
                "  body: new URLSearchParams({}),",
                js_string(&crate::model::urlencode_pairs(&pairs))
            ));
        }
        _ => {
            if !spec.body.is_empty() {
                lines.push(format!("  body: {},", js_string(&spec.body)));
            }
        }
    }

    if spec.transport.follow_redirects {
        lines.push("  redirect: \"follow\",".to_owned());
    } else {
        lines.push("  redirect: \"manual\",".to_owned());
    }
    lines.push("});".to_owned());
    lines.push("const data = await res.json();".to_owned());

    // the FormData has to exist before the call that uses it
    if spec.body_kind == BodyKind::Multipart {
        let mut head = vec!["const form = new FormData();".to_owned()];
        for part in spec.form_parts.iter().filter(|p| p.active()) {
            if part.is_file() {
                head.push(format!(
                    "form.append({}, /* file */ {});",
                    js_string(part.key.trim()),
                    js_string(part.file.trim())
                ));
            } else {
                head.push(format!(
                    "form.append({}, {});",
                    js_string(part.key.trim()),
                    js_string(&part.value)
                ));
            }
        }
        head.push(String::new());
        head.extend(lines);
        lines = head;
    }

    lines.join("\n")
}

fn python(spec: &RequestSpec, insecure_tls: bool, timeout_secs: Option<u64>) -> String {
    let mut lines = vec!["import requests".to_owned(), String::new()];
    for note in unsupported(spec) {
        lines.push(format!("# not expressed below: {note}"));
    }

    let headers = headers_of(spec);
    if !headers.is_empty() {
        lines.push("headers = {".to_owned());
        for (k, v) in &headers {
            lines.push(format!("    {}: {},", py_string(k), py_string(v)));
        }
        lines.push("}".to_owned());
    }

    let mut args = vec![py_string(&spec.full_url())];
    if !headers.is_empty() {
        args.push("headers=headers".to_owned());
    }

    match spec.body_kind {
        BodyKind::None | BodyKind::Binary => {}
        BodyKind::Multipart => {
            lines.push("files = {".to_owned());
            for part in spec.form_parts.iter().filter(|p| p.active()) {
                let value = if part.is_file() {
                    format!("open({}, \"rb\")", py_string(part.file.trim()))
                } else {
                    format!("(None, {})", py_string(&part.value))
                };
                lines.push(format!("    {}: {},", py_string(part.key.trim()), value));
            }
            lines.push("}".to_owned());
            args.push("files=files".to_owned());
        }
        BodyKind::Form => {
            let pairs = crate::model::parse_query(&spec.body.replace(['\n', '\r'], "&"));
            lines.push("data = {".to_owned());
            for kv in pairs.iter().filter(|p| p.active()) {
                lines.push(format!(
                    "    {}: {},",
                    py_string(kv.key.trim()),
                    py_string(&kv.value)
                ));
            }
            lines.push("}".to_owned());
            args.push("data=data".to_owned());
        }
        BodyKind::Json => {
            if !spec.body.is_empty() {
                lines.push(format!("payload = {}", py_string(&spec.body)));
                // requests would re-encode a dict; the text is sent as typed
                args.push("data=payload".to_owned());
            }
        }
        BodyKind::GraphQl => {
            lines.push(format!(
                "payload = {}",
                py_string(&crate::model::graphql_body(&spec.body, &spec.graphql_vars))
            ));
            args.push("data=payload".to_owned());
        }
        _ => {
            if !spec.body.is_empty() {
                lines.push(format!("payload = {}", py_string(&spec.body)));
                args.push("data=payload".to_owned());
            }
        }
    }

    if let Some(secs) = timeout_secs {
        args.push(format!("timeout={secs}"));
    }
    if insecure_tls {
        args.push("verify=False".to_owned());
    }
    if !spec.transport.follow_redirects {
        args.push("allow_redirects=False".to_owned());
    }
    if !spec.transport.proxy.trim().is_empty() {
        let p = py_string(spec.transport.proxy.trim());
        lines.push(format!("proxies = {{\"http\": {p}, \"https\": {p}}}"));
        args.push("proxies=proxies".to_owned());
    }

    if !lines.is_empty() && lines.last().is_some_and(|l| !l.is_empty()) {
        lines.push(String::new());
    }
    lines.push(format!(
        "res = requests.{}({})",
        spec.method.as_str().to_ascii_lowercase(),
        args.join(", ")
    ));
    lines.push("res.raise_for_status()".to_owned());
    lines.push("print(res.json())".to_owned());
    lines.join("\n")
}

fn httpie(spec: &RequestSpec, insecure_tls: bool) -> String {
    let mut parts: Vec<String> = vec!["http".to_owned()];
    if insecure_tls {
        parts.push("--verify=no".to_owned());
    }
    if spec.transport.follow_redirects {
        parts.push("--follow".to_owned());
    }
    if spec.body_kind == BodyKind::Form {
        parts.push("--form".to_owned());
    }
    parts.push(spec.method.as_str().to_owned());
    parts.push(shell_quote(&spec.full_url()));

    for (k, v) in headers_of(spec) {
        parts.push(shell_quote(&format!("{k}:{v}")));
    }

    match spec.body_kind {
        BodyKind::Form => {
            let pairs = crate::model::parse_query(&spec.body.replace(['\n', '\r'], "&"));
            for kv in pairs.iter().filter(|p| p.active()) {
                parts.push(shell_quote(&format!("{}={}", kv.key.trim(), kv.value)));
            }
        }
        BodyKind::Multipart => {
            for part in spec.form_parts.iter().filter(|p| p.active()) {
                let pair = if part.is_file() {
                    format!("{}@{}", part.key.trim(), part.file.trim())
                } else {
                    format!("{}={}", part.key.trim(), part.value)
                };
                parts.push(shell_quote(&pair));
            }
        }
        _ => {}
    }

    let mut out = parts.join(" ");
    // httpie reads a raw body from stdin, which is a pipe rather than an arg
    let raw_body = matches!(spec.body_kind, BodyKind::Json | BodyKind::Xml | BodyKind::Text)
        && !spec.body.is_empty();
    if raw_body {
        out = format!("echo {} | {out}", shell_quote(&spec.body));
    } else if spec.body_kind == BodyKind::GraphQl {
        let envelope = crate::model::graphql_body(&spec.body, &spec.graphql_vars);
        out = format!("echo {} | {out}", shell_quote(&envelope));
    }
    for note in unsupported(spec) {
        out.push_str(&format!("\n# not expressed above: {note}"));
    }
    out
}

/// Single-quote for a POSIX shell, the way curl's own copy-as does it.
fn shell_quote(text: &str) -> String {
    if !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.:/@=+".contains(c))
    {
        return text.to_owned();
    }
    format!("'{}'", text.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ApiKeyIn, Auth, AuthKind, KeyVal, Method};

    fn spec() -> RequestSpec {
        RequestSpec {
            name: "create user".into(),
            method: Method::Post,
            url: "https://api.example.com/users".into(),
            params: vec![KeyVal {
                on: true,
                key: "verbose".into(),
                value: "1".into(),
            }],
            headers: vec![KeyVal {
                on: true,
                key: "X-Trace".into(),
                value: "abc".into(),
            }],
            body_kind: BodyKind::Json,
            body: "{\"name\":\"ada\"}".into(),
            auth: Auth {
                kind: AuthKind::Bearer,
                token: "t0ken".into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn fetch_carries_url_method_headers_and_body() {
        let out = generate(Target::Fetch, &spec(), false, None);
        assert!(out.contains(r#"fetch("https://api.example.com/users?verbose=1""#), "{out}");
        assert!(out.contains(r#"method: "POST""#), "{out}");
        assert!(out.contains(r#""Authorization": "Bearer t0ken""#), "{out}");
        assert!(out.contains(r#""X-Trace": "abc""#), "{out}");
        assert!(out.contains(r#""Content-Type": "application/json""#), "{out}");
        assert!(out.contains(r#"body: "{\"name\":\"ada\"}""#), "{out}");
    }

    #[test]
    fn python_emits_a_runnable_call() {
        let out = generate(Target::Python, &spec(), true, Some(30));
        assert!(out.starts_with("import requests"), "{out}");
        assert!(out.contains("requests.post("), "{out}");
        assert!(out.contains("headers=headers"), "{out}");
        assert!(out.contains("timeout=30"), "{out}");
        assert!(out.contains("verify=False"), "{out}");
        // following redirects is the default, so nothing needs saying
        assert!(!out.contains("allow_redirects"), "{out}");
    }

    #[test]
    fn redirect_handling_is_only_spelled_out_when_it_is_off() {
        let mut s = spec();
        s.transport.follow_redirects = false;

        let py = generate(Target::Python, &s, false, None);
        assert!(py.contains("allow_redirects=False"), "{py}");

        let js = generate(Target::Fetch, &s, false, None);
        assert!(js.contains(r#"redirect: "manual""#), "{js}");

        let h = generate(Target::Httpie, &s, false, None);
        assert!(!h.contains("--follow"), "{h}");
        // and the default direction
        assert!(generate(Target::Httpie, &spec(), false, None).contains("--follow"));
    }

    #[test]
    fn httpie_quotes_headers_as_name_colon_value() {
        let out = generate(Target::Httpie, &spec(), false, None);
        assert!(out.contains(" POST "), "{out}");
        assert!(out.contains("'Authorization:Bearer t0ken'"), "{out}");
        assert!(out.contains("X-Trace:abc"), "{out}");
        // a raw json body is piped in, not passed as an argument
        assert!(out.starts_with("echo "), "{out}");
    }

    #[test]
    fn a_graphql_body_is_sent_as_the_envelope_not_the_bare_query() {
        let mut s = spec();
        s.body_kind = BodyKind::GraphQl;
        s.body = "query Me { viewer { id } }".into();
        s.graphql_vars = r#"{"id": 7}"#.into();

        for target in Target::ALL {
            let out = generate(target, &s, false, None);
            assert!(out.contains(r#"query"#), "{target:?}: {out}");
            assert!(out.contains("viewer"), "{target:?}: {out}");
            assert!(out.contains("variables"), "{target:?}: {out}");
            assert!(out.contains("application/json"), "{target:?}: {out}");
        }
    }

    #[test]
    fn an_explicit_header_row_beats_the_body_kind_default() {
        let mut s = spec();
        s.headers.push(KeyVal {
            on: true,
            key: "content-type".into(),
            value: "application/vnd.api+json".into(),
        });
        let out = generate(Target::Fetch, &s, false, None);
        assert!(out.contains("application/vnd.api+json"), "{out}");
        assert_eq!(out.matches("ontent-Type").count() + out.matches("ontent-type").count(), 1, "{out}");
    }

    #[test]
    fn form_bodies_become_the_idiomatic_thing_in_each_language() {
        let mut s = spec();
        s.body_kind = BodyKind::Form;
        s.body = "a=1\nb=two words".into();

        let js = generate(Target::Fetch, &s, false, None);
        assert!(js.contains("new URLSearchParams("), "{js}");

        let py = generate(Target::Python, &s, false, None);
        assert!(py.contains("data = {"), "{py}");
        assert!(py.contains("\"b\": \"two words\""), "{py}");
        assert!(py.contains("data=data"), "{py}");

        let h = generate(Target::Httpie, &s, false, None);
        assert!(h.contains("--form"), "{h}");
        assert!(h.contains("'b=two words'"), "{h}");
    }

    #[test]
    fn what_a_target_cannot_express_is_said_out_loud() {
        let mut s = spec();
        s.body_kind = BodyKind::Binary;
        s.binary_path = "/tmp/blob.bin".into();
        for target in [Target::Fetch, Target::Python, Target::Httpie] {
            let out = generate(target, &s, false, None);
            assert!(out.contains("/tmp/blob.bin"), "{target:?}: {out}");
            assert!(out.contains("not expressed"), "{target:?}: {out}");
        }
    }

    #[test]
    fn an_api_key_in_the_query_reaches_the_url_not_the_headers() {
        let mut s = spec();
        s.auth = Auth {
            kind: AuthKind::ApiKey,
            api_key_name: "api_key".into(),
            api_key_value: "k3y".into(),
            api_key_in: ApiKeyIn::Query,
            ..Default::default()
        };
        let out = generate(Target::Fetch, &s, false, None);
        assert!(out.contains("api_key=k3y"), "{out}");
        assert!(!out.contains("Authorization"), "{out}");
    }

    #[test]
    fn single_quotes_in_a_value_survive_httpie() {
        let mut s = spec();
        s.headers = vec![KeyVal {
            on: true,
            key: "X-Note".into(),
            value: "it's fine".into(),
        }];
        let out = generate(Target::Httpie, &s, false, None);
        assert!(out.contains(r"'X-Note:it'\''s fine'"), "{out}");
    }
}
