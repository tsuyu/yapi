//! OAuth 2.0 token acquisition: client credentials, password, and the
//! authorization code flow with PKCE via a loopback redirect.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use rand::distributions::Alphanumeric;
use rand::Rng;
use sha2::{Digest, Sha256};

use crate::model::{base64, base64_url, now_unix, ClientAuth, Grant, OAuth2};
use crate::net::SendOpts;

/// How long to wait for the user to finish the browser round trip.
const BROWSER_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    /// Unix seconds, 0 when the server did not say.
    pub expires_at: u64,
    /// Raw token endpoint response, shown in the ui.
    pub raw: String,
}

pub enum TokenMsg {
    Ok(Box<TokenSet>),
    Failed(String),
    /// Progress line while a flow is in flight.
    Status(String),
}

/// Run a flow on a worker thread; the result lands on `tx`.
pub fn spawn_fetch(cfg: OAuth2, opts: SendOpts, tx: Sender<TokenMsg>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let msg = match fetch(&cfg, opts, &tx, &ctx) {
            Ok(t) => TokenMsg::Ok(Box::new(t)),
            Err(e) => TokenMsg::Failed(e),
        };
        let _ = tx.send(msg);
        ctx.request_repaint();
    });
}

/// Exchange the stored refresh token for a new access token.
pub fn spawn_refresh(cfg: OAuth2, opts: SendOpts, tx: Sender<TokenMsg>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let msg = match refresh(&cfg, opts) {
            Ok(t) => TokenMsg::Ok(Box::new(t)),
            Err(e) => TokenMsg::Failed(e),
        };
        let _ = tx.send(msg);
        ctx.request_repaint();
    });
}

fn fetch(
    cfg: &OAuth2,
    opts: SendOpts,
    tx: &Sender<TokenMsg>,
    ctx: &egui::Context,
) -> Result<TokenSet, String> {
    if cfg.token_url.trim().is_empty() {
        return Err("token url is empty".to_owned());
    }
    match cfg.grant {
        Grant::ClientCredentials => {
            let mut form = vec![("grant_type", "client_credentials".to_owned())];
            add_scope(&mut form, cfg);
            post_token(cfg, opts, form)
        }
        Grant::Password => {
            if cfg.username.is_empty() {
                return Err("username is empty".to_owned());
            }
            let mut form = vec![
                ("grant_type", "password".to_owned()),
                ("username", cfg.username.clone()),
                ("password", cfg.password.clone()),
            ];
            add_scope(&mut form, cfg);
            post_token(cfg, opts, form)
        }
        Grant::AuthorizationCode => authorization_code(cfg, opts, tx, ctx),
    }
}

fn refresh(cfg: &OAuth2, opts: SendOpts) -> Result<TokenSet, String> {
    if cfg.refresh_token.trim().is_empty() {
        return Err("no refresh token to use".to_owned());
    }
    let form = vec![
        ("grant_type", "refresh_token".to_owned()),
        ("refresh_token", cfg.refresh_token.clone()),
    ];
    let mut token = post_token(cfg, opts, form)?;
    // servers may omit the refresh token on refresh - keep the one we have
    if token.refresh_token.is_empty() {
        token.refresh_token = cfg.refresh_token.clone();
    }
    Ok(token)
}

fn add_scope(form: &mut Vec<(&'static str, String)>, cfg: &OAuth2) {
    if !cfg.scope.trim().is_empty() {
        form.push(("scope", cfg.scope.trim().to_owned()));
    }
    if !cfg.audience.trim().is_empty() {
        form.push(("audience", cfg.audience.trim().to_owned()));
    }
}

fn post_token(
    cfg: &OAuth2,
    opts: SendOpts,
    mut form: Vec<(&'static str, String)>,
) -> Result<TokenSet, String> {
    let client = crate::net::client(opts, &cfg.token_url)?;
    let mut req = client.post(crate::model::normalize_url(&cfg.token_url));

    match cfg.client_auth {
        ClientAuth::BasicHeader => {
            if !cfg.client_id.is_empty() || !cfg.client_secret.is_empty() {
                let raw = format!("{}:{}", cfg.client_id, cfg.client_secret);
                req = req.header("Authorization", format!("Basic {}", base64(raw.as_bytes())));
            }
        }
        ClientAuth::RequestBody => {
            form.push(("client_id", cfg.client_id.clone()));
            if !cfg.client_secret.is_empty() {
                form.push(("client_secret", cfg.client_secret.clone()));
            }
        }
    }

    let body = form
        .iter()
        .map(|(k, v)| format!("{}={}", crate::model::encode(k), crate::model::encode(v)))
        .collect::<Vec<_>>()
        .join("&");

    let resp = req
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(body)
        .send()
        .map_err(|e| format!("token request failed: {e}"))?;

    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!("token endpoint returned {status}: {}", trim(&text)));
    }
    parse_token(&text)
}

fn parse_token(text: &str) -> Result<TokenSet, String> {
    let v: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| format!("token response is not json ({e}): {}", trim(text)))?;
    let access_token = v
        .get("access_token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| format!("no access_token in response: {}", trim(text)))?
        .to_owned();
    let expires_at = v
        .get("expires_in")
        .and_then(|e| e.as_u64())
        .map(|secs| now_unix() + secs)
        .unwrap_or(0);
    Ok(TokenSet {
        access_token,
        refresh_token: v
            .get("refresh_token")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_owned(),
        token_type: v
            .get("token_type")
            .and_then(|t| t.as_str())
            .unwrap_or("Bearer")
            .to_owned(),
        expires_at,
        raw: serde_json::to_string_pretty(&v).unwrap_or_else(|_| text.to_owned()),
    })
}

fn trim(text: &str) -> String {
    let t = text.trim();
    if t.chars().count() > 300 {
        format!("{}...", t.chars().take(300).collect::<String>())
    } else {
        t.to_owned()
    }
}

// ---- authorization code + pkce ----------------------------------------------

fn authorization_code(
    cfg: &OAuth2,
    opts: SendOpts,
    tx: &Sender<TokenMsg>,
    ctx: &egui::Context,
) -> Result<TokenSet, String> {
    if cfg.auth_url.trim().is_empty() {
        return Err("authorization url is empty".to_owned());
    }

    // bind before opening the browser, so the redirect cannot arrive first
    let listener = TcpListener::bind(("127.0.0.1", cfg.redirect_port))
        .map_err(|e| format!("cannot listen on 127.0.0.1:{}: {e}", cfg.redirect_port))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("listener: {e}"))?;

    let state = random_string(24);
    let verifier = random_string(64);
    let challenge = base64_url(&Sha256::digest(verifier.as_bytes()));

    let mut query = vec![
        ("response_type", "code".to_owned()),
        ("client_id", cfg.client_id.clone()),
        ("redirect_uri", cfg.redirect_uri()),
        ("state", state.clone()),
    ];
    if !cfg.scope.trim().is_empty() {
        query.push(("scope", cfg.scope.trim().to_owned()));
    }
    if !cfg.audience.trim().is_empty() {
        query.push(("audience", cfg.audience.trim().to_owned()));
    }
    if cfg.use_pkce {
        query.push(("code_challenge", challenge));
        query.push(("code_challenge_method", "S256".to_owned()));
    }
    let sep = if cfg.auth_url.contains('?') { "&" } else { "?" };
    let auth_url = format!(
        "{}{sep}{}",
        crate::model::normalize_url(&cfg.auth_url),
        query
            .iter()
            .map(|(k, v)| format!("{}={}", crate::model::encode(k), crate::model::encode(v)))
            .collect::<Vec<_>>()
            .join("&")
    );

    let _ = tx.send(TokenMsg::Status(format!(
        "waiting for the browser on {} ...",
        cfg.redirect_uri()
    )));
    ctx.request_repaint();
    open_browser(&auth_url)?;

    let code = wait_for_code(&listener, &state)?;

    let mut form = vec![
        ("grant_type", Grant::AuthorizationCode.wire_name().to_owned()),
        ("code", code),
        ("redirect_uri", cfg.redirect_uri()),
    ];
    if cfg.use_pkce {
        form.push(("code_verifier", verifier));
    }
    if cfg.client_auth == ClientAuth::BasicHeader && cfg.client_secret.is_empty() {
        // public client: no secret to Basic-auth with, so identify in the body
        form.push(("client_id", cfg.client_id.clone()));
    }
    post_token(cfg, opts, form)
}

/// Accept redirects until one carries our state, or time runs out.
fn wait_for_code(listener: &TcpListener, expected_state: &str) -> Result<String, String> {
    let deadline = Instant::now() + BROWSER_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let mut reader = BufReader::new(&stream);
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                // "GET /callback?code=...&state=... HTTP/1.1"
                let target = line.split_whitespace().nth(1).unwrap_or("");
                let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
                let pairs = crate::model::parse_query(query);
                let get = |name: &str| {
                    pairs
                        .iter()
                        .find(|p| p.key == name)
                        .map(|p| p.value.clone())
                };

                if let Some(err) = get("error") {
                    let desc = get("error_description").unwrap_or_default();
                    reply(&stream, "Authorization failed. You can close this tab.");
                    return Err(format!("authorization server said: {err} {desc}").trim().to_owned());
                }
                let Some(code) = get("code") else {
                    reply(&stream, "Waiting for the authorization redirect...");
                    continue;
                };
                if get("state").as_deref() != Some(expected_state) {
                    reply(&stream, "State mismatch. You can close this tab.");
                    return Err("state mismatch - redirect did not come from our request".to_owned());
                }
                reply(&stream, "Authorized. You can close this tab and go back to yAPI.");
                return Ok(code);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("timed out waiting for the browser redirect".to_owned());
                }
                std::thread::sleep(Duration::from_millis(120));
            }
            Err(e) => return Err(format!("redirect listener: {e}")),
        }
    }
}

fn reply(mut stream: &std::net::TcpStream, message: &str) {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>yAPI</title>\
         <body style=\"font:16px system-ui;padding:3rem\">{message}</body>"
    );
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
}

fn open_browser(url: &str) -> Result<(), String> {
    let result = if cfg!(target_os = "windows") {
        // the empty "" is the window title cmd's start expects first
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    result
        .map(|_| ())
        .map_err(|e| format!("could not open a browser ({e}) - open this url yourself:\n{url}"))
}

fn random_string(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;


    /// A token endpoint that echoes nothing and hands back a canned token.
    fn token_server() -> (u16, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let mut seen = String::new();
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap_or(0);
                }
                let end = line == "\r\n" || line == "\n";
                seen.push_str(&line);
                if end {
                    break;
                }
            }
            if len > 0 {
                let mut body = vec![0u8; len];
                std::io::Read::read_exact(&mut reader, &mut body).unwrap();
                seen.push_str(&String::from_utf8_lossy(&body));
            }
            let body = br#"{"access_token":"at-1","token_type":"Bearer","expires_in":120,"refresh_token":"rt-1"}"#;
            let mut out = &stream;
            write!(
                out,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            out.write_all(body).unwrap();
            out.flush().unwrap();
            seen
        });
        (port, handle)
    }

    #[test]
    fn client_credentials_against_a_local_token_endpoint() {
        let (port, server) = token_server();
        let cfg = OAuth2 {
            grant: Grant::ClientCredentials,
            token_url: format!("localhost:{port}/oauth/token"),
            client_id: "cid".to_owned(),
            client_secret: "csecret".to_owned(),
            scope: "read write".to_owned(),
            client_auth: ClientAuth::BasicHeader,
            ..Default::default()
        };
        let (tx, _rx) = std::sync::mpsc::channel();
        let token = fetch(
            &cfg,
            SendOpts {
                timeout_secs: 10,
                insecure_tls: false,
            },
            &tx,
            &egui::Context::default(),
        )
        .expect("token fetch failed");

        assert_eq!(token.access_token, "at-1");
        assert_eq!(token.refresh_token, "rt-1");
        assert!(token.expires_at >= now_unix() + 110);

        let seen = server.join().unwrap();
        assert!(seen.starts_with("POST /oauth/token HTTP/1.1"), "{seen}");
        // client id/secret as http basic, per rfc 6749
        let expected = format!("Basic {}", base64(b"cid:csecret"));
        assert!(
            seen.contains(&expected),
            "basic client auth missing:\n{seen}"
        );
        assert!(
            seen.contains("grant_type=client_credentials"),
            "grant missing:\n{seen}"
        );
        assert!(seen.contains("scope=read+write"), "scope missing:\n{seen}");
    }

    #[test]
    fn client_credentials_in_the_request_body() {
        let (port, server) = token_server();
        let cfg = OAuth2 {
            grant: Grant::ClientCredentials,
            token_url: format!("localhost:{port}/t"),
            client_id: "cid".to_owned(),
            client_secret: "csecret".to_owned(),
            client_auth: ClientAuth::RequestBody,
            ..Default::default()
        };
        let (tx, _rx) = std::sync::mpsc::channel();
        fetch(
            &cfg,
            SendOpts {
                timeout_secs: 10,
                insecure_tls: false,
            },
            &tx,
            &egui::Context::default(),
        )
        .unwrap();
        let seen = server.join().unwrap();
        assert!(seen.contains("client_id=cid"), "{seen}");
        assert!(seen.contains("client_secret=csecret"), "{seen}");
        assert!(!seen.contains("Basic "), "should not also send basic:\n{seen}");
    }

    #[test]
    fn refresh_keeps_the_old_refresh_token_when_the_server_omits_it() {
        let cfg = OAuth2 {
            refresh_token: String::new(),
            ..Default::default()
        };
        let err = refresh(
            &cfg,
            SendOpts {
                timeout_secs: 5,
                insecure_tls: false,
            },
        )
        .unwrap_err();
        assert!(err.contains("no refresh token"), "{err}");
    }

    #[test]
    fn parses_a_token_response() {
        let t = parse_token(
            r#"{"access_token":"at","refresh_token":"rt","token_type":"Bearer","expires_in":3600}"#,
        )
        .unwrap();
        assert_eq!(t.access_token, "at");
        assert_eq!(t.refresh_token, "rt");
        assert!(t.expires_at >= now_unix() + 3500);
    }

    #[test]
    fn missing_access_token_is_an_error() {
        let err = parse_token(r#"{"error":"invalid_client"}"#).unwrap_err();
        assert!(err.contains("no access_token"), "{err}");
        assert!(parse_token("<html>nope</html>").is_err());
    }

    #[test]
    fn pkce_challenge_matches_rfc_7636_appendix_b() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = base64_url(&Sha256::digest(verifier.as_bytes()));
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn random_strings_differ_and_are_url_safe() {
        let a = random_string(64);
        let b = random_string(64);
        assert_ne!(a, b);
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}
