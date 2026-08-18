use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl Method {
    pub const ALL: [Method; 7] = [
        Method::Get,
        Method::Post,
        Method::Put,
        Method::Patch,
        Method::Delete,
        Method::Head,
        Method::Options,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
            Method::Head => "HEAD",
            Method::Options => "OPTIONS",
        }
    }

    pub fn color(self) -> egui::Color32 {
        match self {
            Method::Get => egui::Color32::from_rgb(0x4c, 0xaf, 0x50),
            Method::Post => egui::Color32::from_rgb(0xff, 0xa7, 0x26),
            Method::Put => egui::Color32::from_rgb(0x42, 0xa5, 0xf5),
            Method::Patch => egui::Color32::from_rgb(0xab, 0x47, 0xbc),
            Method::Delete => egui::Color32::from_rgb(0xef, 0x53, 0x50),
            Method::Head | Method::Options => egui::Color32::GRAY,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BodyKind {
    None,
    Json,
    Xml,
    Text,
    Form,
    Multipart,
    Binary,
}

impl BodyKind {
    pub const ALL: [BodyKind; 7] = [
        BodyKind::None,
        BodyKind::Json,
        BodyKind::Xml,
        BodyKind::Text,
        BodyKind::Form,
        BodyKind::Multipart,
        BodyKind::Binary,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            BodyKind::None => "none",
            BodyKind::Json => "json",
            BodyKind::Xml => "xml",
            BodyKind::Text => "raw",
            BodyKind::Form => "x-www-form-urlencoded",
            BodyKind::Multipart => "form-data",
            BodyKind::Binary => "binary",
        }
    }

    /// Content-Type sent unless the headers tab overrides it.
    pub fn content_type(self) -> Option<&'static str> {
        match self {
            BodyKind::None | BodyKind::Multipart | BodyKind::Binary => None,
            BodyKind::Json => Some("application/json"),
            BodyKind::Xml => Some("application/xml"),
            BodyKind::Text => Some("text/plain; charset=utf-8"),
            BodyKind::Form => Some("application/x-www-form-urlencoded"),
        }
    }

    /// Does this kind use the plain text editor?
    pub fn is_text(self) -> bool {
        matches!(self, BodyKind::Json | BodyKind::Xml | BodyKind::Text)
    }
}

/// One row of a `form-data` body: either a text field or a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FormPart {
    pub on: bool,
    pub key: String,
    pub value: String,
    /// When set, `value` is ignored and this file is uploaded instead.
    pub file: String,
    pub content_type: String,
}

impl Default for FormPart {
    fn default() -> Self {
        Self {
            on: true,
            key: String::new(),
            value: String::new(),
            file: String::new(),
            content_type: String::new(),
        }
    }
}

impl FormPart {
    pub fn active(&self) -> bool {
        self.on && !self.key.trim().is_empty()
    }

    pub fn is_file(&self) -> bool {
        !self.file.trim().is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KeyVal {
    pub on: bool,
    pub key: String,
    pub value: String,
}

impl Default for KeyVal {
    fn default() -> Self {
        Self {
            on: true,
            key: String::new(),
            value: String::new(),
        }
    }
}

impl KeyVal {
    pub fn active(&self) -> bool {
        self.on && !self.key.trim().is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthKind {
    None,
    Bearer,
    Basic,
    ApiKey,
    OAuth2,
    Jwt,
}

impl AuthKind {
    pub const ALL: [AuthKind; 6] = [
        AuthKind::None,
        AuthKind::Bearer,
        AuthKind::Basic,
        AuthKind::ApiKey,
        AuthKind::OAuth2,
        AuthKind::Jwt,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            AuthKind::None => "none",
            AuthKind::Bearer => "bearer",
            AuthKind::Basic => "basic",
            AuthKind::ApiKey => "api key",
            AuthKind::OAuth2 => "oauth 2.0",
            AuthKind::Jwt => "jwt",
        }
    }
}

/// Which oauth2 flow to run to get a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Grant {
    /// Browser round trip to the authorization endpoint, PKCE by default.
    AuthorizationCode,
    ClientCredentials,
    /// Resource owner password credentials.
    Password,
}

impl Grant {
    pub const ALL: [Grant; 3] = [
        Grant::AuthorizationCode,
        Grant::ClientCredentials,
        Grant::Password,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Grant::AuthorizationCode => "authorization code",
            Grant::ClientCredentials => "client credentials",
            Grant::Password => "password",
        }
    }

    pub fn wire_name(self) -> &'static str {
        match self {
            Grant::AuthorizationCode => "authorization_code",
            Grant::ClientCredentials => "client_credentials",
            Grant::Password => "password",
        }
    }
}

/// How client credentials reach the token endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientAuth {
    /// HTTP Basic, per RFC 6749 - what most servers expect.
    BasicHeader,
    /// client_id / client_secret as form fields.
    RequestBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OAuth2 {
    pub grant: Grant,
    pub auth_url: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub scope: String,
    pub audience: String,
    pub username: String,
    pub password: String,
    pub client_auth: ClientAuth,
    pub use_pkce: bool,
    /// Loopback port the browser is redirected back to.
    pub redirect_port: u16,
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    /// Unix seconds when the access token expires; 0 when unknown.
    pub expires_at: u64,
}

impl Default for OAuth2 {
    fn default() -> Self {
        Self {
            grant: Grant::ClientCredentials,
            auth_url: String::new(),
            token_url: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
            scope: String::new(),
            audience: String::new(),
            username: String::new(),
            password: String::new(),
            client_auth: ClientAuth::BasicHeader,
            use_pkce: true,
            redirect_port: 8721,
            access_token: String::new(),
            refresh_token: String::new(),
            token_type: "Bearer".to_owned(),
            expires_at: 0,
        }
    }
}

impl OAuth2 {
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.redirect_port)
    }

    pub fn seconds_left(&self) -> Option<i64> {
        if self.expires_at == 0 {
            return None;
        }
        Some(self.expires_at as i64 - now_unix() as i64)
    }

    pub fn expired(&self) -> bool {
        self.seconds_left().is_some_and(|s| s <= 0)
    }
}

/// A JWT used as a credential: paste one, or sign one with HS256.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct JwtAuth {
    pub token: String,
    /// Header the token rides in. Empty means `Authorization`.
    pub header_name: String,
    /// Prefix before the token. Empty means no prefix.
    pub prefix: String,
    /// Signing inputs, used by the "sign" button.
    pub claims: String,
    pub secret: String,
    pub secret_is_base64: bool,
}

impl Default for JwtAuth {
    fn default() -> Self {
        Self {
            token: String::new(),
            header_name: "Authorization".to_owned(),
            prefix: "Bearer".to_owned(),
            claims: "{\n  \"sub\": \"1234567890\",\n  \"name\": \"dev\"\n}".to_owned(),
            secret: String::new(),
            secret_is_base64: false,
        }
    }
}

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Where an api key rides: a header, or a query param.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApiKeyIn {
    Header,
    Query,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Auth {
    pub kind: AuthKind,
    pub token: String,
    pub username: String,
    pub password: String,
    pub api_key_name: String,
    pub api_key_value: String,
    pub api_key_in: ApiKeyIn,
    pub oauth: OAuth2,
    pub jwt: JwtAuth,
}

impl Default for Auth {
    fn default() -> Self {
        Self {
            kind: AuthKind::None,
            token: String::new(),
            username: String::new(),
            password: String::new(),
            api_key_name: "X-API-Key".to_owned(),
            api_key_value: String::new(),
            api_key_in: ApiKeyIn::Header,
            oauth: OAuth2::default(),
            jwt: JwtAuth::default(),
        }
    }
}

impl Auth {
    /// The header this auth adds, if any.
    pub fn header(&self) -> Option<(String, String)> {
        match self.kind {
            AuthKind::None => None,
            AuthKind::Bearer => {
                let t = self.token.trim();
                if t.is_empty() {
                    None
                } else if t.to_ascii_lowercase().starts_with("bearer ") {
                    // user pasted the whole header value - don't double the prefix
                    Some(("Authorization".to_owned(), t.to_owned()))
                } else {
                    Some(("Authorization".to_owned(), format!("Bearer {t}")))
                }
            }
            AuthKind::Basic => {
                if self.username.is_empty() && self.password.is_empty() {
                    None
                } else {
                    let raw = format!("{}:{}", self.username, self.password);
                    Some((
                        "Authorization".to_owned(),
                        format!("Basic {}", base64(raw.as_bytes())),
                    ))
                }
            }
            AuthKind::OAuth2 => {
                let token = self.oauth.access_token.trim();
                if token.is_empty() {
                    None
                } else {
                    let scheme = match self.oauth.token_type.trim() {
                        "" => "Bearer",
                        other => other,
                    };
                    Some(("Authorization".to_owned(), format!("{scheme} {token}")))
                }
            }
            AuthKind::Jwt => {
                let token = self.jwt.token.trim();
                if token.is_empty() {
                    return None;
                }
                let name = match self.jwt.header_name.trim() {
                    "" => "Authorization",
                    other => other,
                };
                let value = match self.jwt.prefix.trim() {
                    "" => token.to_owned(),
                    prefix => format!("{prefix} {token}"),
                };
                Some((name.to_owned(), value))
            }
            AuthKind::ApiKey => {
                if self.api_key_in != ApiKeyIn::Header {
                    return None;
                }
                let name = self.api_key_name.trim();
                if name.is_empty() || self.api_key_value.is_empty() {
                    None
                } else {
                    Some((name.to_owned(), self.api_key_value.clone()))
                }
            }
        }
    }

    /// The query param this auth adds, if any (api key in query mode).
    pub fn query_param(&self) -> Option<(String, String)> {
        if self.kind != AuthKind::ApiKey || self.api_key_in != ApiKeyIn::Query {
            return None;
        }
        let name = self.api_key_name.trim();
        if name.is_empty() || self.api_key_value.is_empty() {
            None
        } else {
            Some((name.to_owned(), self.api_key_value.clone()))
        }
    }

    /// One-line summary for the tab label.
    pub fn summary(&self) -> &'static str {
        match self.kind {
            AuthKind::None => "auth",
            AuthKind::Bearer => "auth (bearer)",
            AuthKind::Basic => "auth (basic)",
            AuthKind::ApiKey => "auth (api key)",
            AuthKind::OAuth2 => "auth (oauth 2.0)",
            AuthKind::Jwt => "auth (jwt)",
        }
    }
}

/// Unpadded base64url, as used by jwt and pkce.
pub fn base64_url(input: &[u8]) -> String {
    base64(input)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

pub fn base64_url_decode(input: &str) -> Result<Vec<u8>, String> {
    const REV: fn(u8) -> Option<u8> = |c: u8| match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    };
    let mut bits: u32 = 0;
    let mut nbits = 0;
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    for c in input.bytes().filter(|c| *c != b'=') {
        let v = REV(c).ok_or_else(|| format!("not base64: {:?}", c as char))?;
        bits = (bits << 6) | v as u32;
        nbits += 6;
        if nbits >= 8 {
            nbits -= 8;
            out.push((bits >> nbits) as u8);
        }
    }
    Ok(out)
}

pub fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RequestSpec {
    pub name: String,
    pub method: Method,
    pub url: String,
    pub params: Vec<KeyVal>,
    /// `:name` / `{name}` placeholders substituted into the url path.
    pub path_params: Vec<KeyVal>,
    pub headers: Vec<KeyVal>,
    pub cookies: Vec<KeyVal>,
    pub body_kind: BodyKind,
    pub body: String,
    pub form_parts: Vec<FormPart>,
    /// File sent as the whole body when `body_kind` is `Binary`.
    pub binary_path: String,
    pub binary_content_type: String,
    pub auth: Auth,
}

impl Default for RequestSpec {
    fn default() -> Self {
        Self {
            name: "new request".to_owned(),
            method: Method::Get,
            url: "https://httpbin.org/get".to_owned(),
            params: vec![KeyVal::default()],
            path_params: Vec::new(),
            headers: vec![KeyVal::default()],
            cookies: vec![KeyVal::default()],
            body_kind: BodyKind::None,
            body: String::new(),
            form_parts: vec![FormPart::default()],
            binary_path: String::new(),
            binary_content_type: String::new(),
            auth: Auth::default(),
        }
    }
}

impl RequestSpec {
    /// Query params merged into the URL, as it will actually be sent.
    pub fn full_url(&self) -> String {
        let base = apply_path_params(&normalize_url(&self.url), &self.path_params);
        let (path, url_query) = split_query(&base);
        let mut parts: Vec<String> = Vec::new();
        if !url_query.is_empty() {
            parts.push(url_query.to_owned());
        }
        for p in self.params.iter().filter(|p| p.active()) {
            parts.push(format!("{}={}", encode(p.key.trim()), encode(&p.value)));
        }
        if let Some((k, v)) = self.auth.query_param() {
            parts.push(format!("{}={}", encode(&k), encode(&v)));
        }
        if parts.is_empty() {
            path.to_owned()
        } else {
            format!("{path}?{}", parts.join("&"))
        }
    }

    /// Cookie header value built from the cookies table.
    pub fn cookie_header(&self) -> Option<String> {
        let jar: Vec<String> = self
            .cookies
            .iter()
            .filter(|c| c.active())
            .map(|c| format!("{}={}", c.key.trim(), c.value.trim()))
            .collect();
        (!jar.is_empty()).then(|| jar.join("; "))
    }

    /// Add a row for every placeholder in the url that has none yet.
    pub fn sync_path_params(&mut self) {
        for name in placeholders(&self.url) {
            if !self.path_params.iter().any(|p| p.key.trim() == name) {
                self.path_params.push(KeyVal {
                    on: true,
                    key: name,
                    value: String::new(),
                });
            }
        }
    }

    /// Placeholders in the url with no value filled in yet.
    pub fn unresolved_path_params(&self) -> Vec<String> {
        placeholders(&self.url)
            .into_iter()
            .filter(|name| {
                !self
                    .path_params
                    .iter()
                    .any(|p| p.active() && p.key.trim() == name && !p.value.is_empty())
            })
            .collect()
    }

    /// Move any `?a=b&c=d` from the URL field into the params table.
    pub fn pull_params_from_url(&mut self) {
        let base = normalize_url(&self.url);
        let (path, query) = split_query(&base);
        if query.is_empty() {
            return;
        }
        let pulled = parse_query(query);
        self.url = path.to_owned();
        self.params.retain(|p| !p.key.trim().is_empty());
        self.params.extend(pulled);
        self.params.push(KeyVal::default());
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Collection {
    pub requests: Vec<RequestSpec>,
}

/// `:name` and `{name}` placeholders found in a url, in order, deduplicated.
pub fn placeholders(url: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let bytes = url.as_bytes();
    // skip the scheme so "http://" does not read as a ":" placeholder
    let mut i = url.find("://").map(|pos| pos + 3).unwrap_or(0);
    while i < bytes.len() {
        match bytes[i] {
            b':' => {
                let start = i + 1;
                let len = url[start..]
                    .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
                    .unwrap_or(url.len() - start);
                let name = &url[start..start + len];
                // ":8080" is a port, not a placeholder
                if !name.is_empty() && !name.chars().all(|c| c.is_ascii_digit()) {
                    push_unique(&mut out, name);
                }
                i = (start + len).max(i + 1);
            }
            b'{' => {
                if let Some(rel) = url[i..].find('}') {
                    let name = url[i + 1..i + rel].trim().trim_matches('{');
                    if !name.is_empty() {
                        push_unique(&mut out, name);
                    }
                    i += rel + 1;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

fn push_unique(out: &mut Vec<String>, name: &str) {
    if !out.iter().any(|n| n == name) {
        out.push(name.to_owned());
    }
}

/// Substitute path params into a url. A placeholder with no value is left as
/// typed, so the gap shows up in the url preview instead of vanishing.
pub fn apply_path_params(url: &str, params: &[KeyVal]) -> String {
    let mut out = url.to_owned();
    for p in params.iter().filter(|p| p.active() && !p.value.is_empty()) {
        let name = p.key.trim();
        let value = encode_path(&p.value);
        out = out
            .replace(&format!("{{{{{name}}}}}"), &value)
            .replace(&format!("{{{name}}}"), &value)
            .replace(&format!(":{name}"), &value);
    }
    out
}

/// Percent-encode a path segment: like `encode`, but a space becomes `%20`
/// rather than `+`, and `/` is left alone.
pub fn encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Fill in a missing scheme. Local dev servers speak http far more often than
/// https, so loopback hosts get `http://` and everything else `https://`.
/// A bare `:3000/api` is treated as localhost.
pub fn normalize_url(url: &str) -> String {
    let u = url.trim();
    if u.is_empty() || u.contains("://") {
        return u.to_owned();
    }
    if let Some(rest) = u.strip_prefix(':') {
        // ":3000/health" -> localhost on that port
        if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return format!("http://localhost:{rest}");
        }
    }
    if is_loopback(u) {
        format!("http://{u}")
    } else {
        format!("https://{u}")
    }
}

/// Host part of a url, with scheme, userinfo, port, path and query stripped.
pub fn host_of(url: &str) -> &str {
    let mut rest = url.trim();
    if let Some((_, after)) = rest.split_once("://") {
        rest = after;
    }
    rest = rest.split(['/', '?', '#']).next().unwrap_or("");
    if let Some((_, after_at)) = rest.rsplit_once('@') {
        rest = after_at;
    }
    if rest.starts_with('[') {
        // ipv6 literal: [::1]:8080
        return rest.split(']').next().map(|h| &h[1..]).unwrap_or(rest);
    }
    rest.split(':').next().unwrap_or(rest)
}

/// Does this url point at this machine?
pub fn is_loopback(url: &str) -> bool {
    let host = host_of(url).to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host == "::1"
        || host == "0.0.0.0"
        || host == "host.docker.internal"
        || host
            .strip_prefix("127.")
            .is_some_and(|rest| rest.split('.').count() == 3)
}

fn split_query(url: &str) -> (&str, &str) {
    match url.split_once('?') {
        Some((p, q)) => (p, q),
        None => (url, ""),
    }
}

pub fn parse_query(query: &str) -> Vec<KeyVal> {
    query
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            KeyVal {
                on: true,
                key: decode(k),
                value: decode(v),
            }
        })
        .collect()
}

/// `key=value` pairs joined with `&`, for form bodies.
pub fn urlencode_pairs(pairs: &[KeyVal]) -> String {
    pairs
        .iter()
        .filter(|p| p.active())
        .map(|p| format!("{}={}", encode(p.key.trim()), encode(&p.value)))
        .collect::<Vec<_>>()
        .join("&")
}

pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_appended_to_url() {
        let mut s = RequestSpec {
            url: "api.example.com/search".to_owned(),
            params: vec![
                KeyVal { on: true, key: "q".into(), value: "hello world".into() },
                KeyVal { on: false, key: "skip".into(), value: "1".into() },
                KeyVal::default(),
            ],
            ..Default::default()
        };
        assert_eq!(s.full_url(), "https://api.example.com/search?q=hello+world");

        s.url = "https://api.example.com/search?page=2".to_owned();
        assert_eq!(
            s.full_url(),
            "https://api.example.com/search?page=2&q=hello+world"
        );
    }

    #[test]
    fn pull_params_out_of_url() {
        let mut s = RequestSpec {
            url: "https://x.dev/a?q=a%20b&n=2".to_owned(),
            params: vec![KeyVal::default()],
            ..Default::default()
        };
        s.pull_params_from_url();
        assert_eq!(s.url, "https://x.dev/a");
        let got: Vec<_> = s
            .params
            .iter()
            .filter(|p| p.active())
            .map(|p| (p.key.clone(), p.value.clone()))
            .collect();
        assert_eq!(
            got,
            vec![("q".to_owned(), "a b".to_owned()), ("n".to_owned(), "2".to_owned())]
        );
    }

    #[test]
    fn encode_decode_roundtrip() {
        let raw = "a b&c=d/é";
        assert_eq!(decode(&encode(raw)), raw);
    }

    #[test]
    fn form_body_encodes_pairs() {
        let pairs = parse_query("user=me&pw=p%40ss");
        assert_eq!(urlencode_pairs(&pairs), "user=me&pw=p%40ss");
    }

    #[test]
    fn base64_known_vectors() {
        assert_eq!(base64(b"user:pass"), "dXNlcjpwYXNz");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(base64(b""), "");
    }

    #[test]
    fn bearer_header() {
        let mut a = Auth {
            kind: AuthKind::Bearer,
            token: "  abc123 ".to_owned(),
            ..Default::default()
        };
        assert_eq!(
            a.header(),
            Some(("Authorization".to_owned(), "Bearer abc123".to_owned()))
        );
        // already-prefixed paste is not doubled
        a.token = "bearer xyz".to_owned();
        assert_eq!(
            a.header(),
            Some(("Authorization".to_owned(), "bearer xyz".to_owned()))
        );
        a.token = "   ".to_owned();
        assert_eq!(a.header(), None);
    }

    #[test]
    fn basic_header() {
        let a = Auth {
            kind: AuthKind::Basic,
            username: "user".to_owned(),
            password: "pass".to_owned(),
            ..Default::default()
        };
        assert_eq!(
            a.header(),
            Some(("Authorization".to_owned(), "Basic dXNlcjpwYXNz".to_owned()))
        );
    }

    #[test]
    fn api_key_header_or_query() {
        let mut a = Auth {
            kind: AuthKind::ApiKey,
            api_key_name: "X-API-Key".to_owned(),
            api_key_value: "k1".to_owned(),
            ..Default::default()
        };
        assert_eq!(a.header(), Some(("X-API-Key".to_owned(), "k1".to_owned())));
        assert_eq!(a.query_param(), None);

        a.api_key_in = ApiKeyIn::Query;
        a.api_key_name = "api_key".to_owned();
        assert_eq!(a.header(), None);
        assert_eq!(a.query_param(), Some(("api_key".to_owned(), "k1".to_owned())));

        let spec = RequestSpec {
            url: "https://x.dev/a".to_owned(),
            params: vec![KeyVal::default()],
            auth: a,
            ..Default::default()
        };
        assert_eq!(spec.full_url(), "https://x.dev/a?api_key=k1");
    }

    #[test]
    fn old_collections_load_without_auth_field() {
        let json = r#"{"requests":[{"name":"old","method":"Get","url":"https://x.dev"}]}"#;
        let c: Collection = serde_json::from_str(json).unwrap();
        assert_eq!(c.requests[0].auth.kind, AuthKind::None);
        assert_eq!(c.requests[0].auth.api_key_name, "X-API-Key");
    }

    #[test]
    fn loopback_hosts_default_to_http() {
        assert_eq!(normalize_url("localhost:3000/api"), "http://localhost:3000/api");
        assert_eq!(normalize_url("127.0.0.1:8080"), "http://127.0.0.1:8080");
        assert_eq!(normalize_url("api.localhost/v1"), "http://api.localhost/v1");
        assert_eq!(normalize_url(":3000/health"), "http://localhost:3000/health");
        assert_eq!(normalize_url("[::1]:9000/x"), "http://[::1]:9000/x");
    }

    #[test]
    fn remote_hosts_still_default_to_https() {
        assert_eq!(normalize_url("api.example.com/v1"), "https://api.example.com/v1");
        // an explicit scheme is never rewritten
        assert_eq!(normalize_url("http://api.example.com"), "http://api.example.com");
        assert_eq!(normalize_url("https://localhost:8443"), "https://localhost:8443");
    }

    #[test]
    fn host_parsing() {
        assert_eq!(host_of("http://user:pw@localhost:3000/a?b=c"), "localhost");
        assert_eq!(host_of("[::1]:8080/x"), "::1");
        assert_eq!(host_of("https://api.example.com"), "api.example.com");
        assert!(is_loopback("http://127.13.2.1:5000"));
        assert!(!is_loopback("http://127.example.com"));
        assert!(!is_loopback("https://api.example.com"));
    }

    #[test]
    fn finds_placeholders_but_not_ports() {
        assert_eq!(
            placeholders("http://localhost:8080/users/:id/posts/{postId}"),
            vec!["id".to_owned(), "postId".to_owned()]
        );
        assert_eq!(placeholders("https://x.dev/a/b"), Vec::<String>::new());
        // repeated placeholder is listed once
        assert_eq!(placeholders("/a/:id/b/:id"), vec!["id".to_owned()]);
    }

    #[test]
    fn substitutes_path_params_and_flags_the_missing() {
        let mut spec = RequestSpec {
            url: "localhost:9000/users/:id/posts/{postId}".to_owned(),
            params: vec![],
            path_params: vec![KeyVal {
                on: true,
                key: "id".into(),
                value: "42".into(),
            }],
            ..Default::default()
        };
        assert_eq!(spec.unresolved_path_params(), vec!["postId".to_owned()]);
        // the unfilled placeholder stays visible rather than vanishing
        assert_eq!(
            spec.full_url(),
            "http://localhost:9000/users/42/posts/{postId}"
        );

        spec.sync_path_params();
        assert!(spec.path_params.iter().any(|p| p.key == "postId"));
        spec.path_params
            .iter_mut()
            .find(|p| p.key == "postId")
            .unwrap()
            .value = "7".into();
        assert_eq!(spec.full_url(), "http://localhost:9000/users/42/posts/7");
        assert!(spec.unresolved_path_params().is_empty());
    }

    #[test]
    fn cookie_header_joins_active_rows() {
        let spec = RequestSpec {
            cookies: vec![
                KeyVal {
                    on: true,
                    key: "a".into(),
                    value: "1".into(),
                },
                KeyVal {
                    on: false,
                    key: "skip".into(),
                    value: "x".into(),
                },
                KeyVal::default(),
            ],
            ..Default::default()
        };
        assert_eq!(spec.cookie_header().as_deref(), Some("a=1"));
        assert_eq!(RequestSpec::default().cookie_header(), None);
    }

    #[test]
    fn base64_url_is_unpadded_and_round_trips() {
        assert_eq!(base64_url(&[251, 255, 190]), "-_--");
        let raw = b"any + old / bytes";
        assert_eq!(base64_url_decode(&base64_url(raw)).unwrap(), raw);
        assert!(base64_url_decode("not base64!").is_err());
    }

    #[test]
    fn oauth_expiry_reads_the_clock() {
        let mut o = OAuth2::default();
        assert_eq!(o.seconds_left(), None);
        assert!(!o.expired(), "unknown expiry is not expired");
        o.expires_at = now_unix() - 1;
        assert!(o.expired());
        o.expires_at = now_unix() + 600;
        assert!(!o.expired());
        assert!(o.seconds_left().unwrap() > 590);
    }
}
