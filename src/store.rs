use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::{Collection, RequestSpec};

/// Everything the app remembers between runs that is not a saved request:
/// the in-progress draft (url, params, headers, auth, body) and which entry
/// was open. Written on edit and on exit, so nothing typed is ever lost.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub draft: RequestSpec,
    pub selected: Option<usize>,
    pub timeout_secs: u64,
    pub insecure_tls: bool,
    /// Variables as they stood, extracted ones included.
    pub vars: std::collections::BTreeMap<String, String>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            draft: RequestSpec::default(),
            selected: None,
            timeout_secs: 30,
            insecure_tls: false,
            vars: std::collections::BTreeMap::new(),
        }
    }
}

/// Where saved requests live by default (per-user config dir).
pub fn default_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "yAPI")
        .map(|d| d.config_dir().join("collection.json"))
        .unwrap_or_else(|| PathBuf::from("collection.json"))
}

pub fn session_path() -> PathBuf {
    default_path().with_file_name("session.json")
}

pub fn load_session(path: &Path) -> Option<Session> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_session(path: &Path, session: &Session) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(session).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

pub fn load(path: &Path) -> Collection {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(path: &Path, coll: &Collection) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string_pretty(coll).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ApiKeyIn, Auth, AuthKind, BodyKind, FormPart, KeyVal, Method};

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("yapi-test-{name}-{}", std::process::id()));
        p.push("collection.json");
        p
    }

    fn loaded_spec() -> RequestSpec {
        RequestSpec {
            name: "everything".to_owned(),
            method: Method::Post,
            url: "https://api.example.com/v1/items".to_owned(),
            params: vec![
                KeyVal { on: true, key: "page".into(), value: "2".into() },
                KeyVal { on: false, key: "debug".into(), value: "1".into() },
            ],
            headers: vec![KeyVal { on: true, key: "Accept".into(), value: "application/json".into() }],
            body_kind: BodyKind::Json,
            body: "{\"a\":1}".to_owned(),
            path_params: vec![KeyVal { on: true, key: "id".into(), value: "42".into() }],
            cookies: vec![KeyVal { on: true, key: "session".into(), value: "abc".into() }],
            form_parts: vec![FormPart {
                on: true,
                key: "upload".into(),
                value: String::new(),
                file: "C:/tmp/x.png".into(),
                content_type: "image/png".into(),
            }],
            binary_path: "C:/tmp/blob.bin".to_owned(),
            binary_content_type: "application/octet-stream".to_owned(),
            auth: Auth {
                kind: AuthKind::ApiKey,
                api_key_name: "api_key".to_owned(),
                api_key_value: "secret".to_owned(),
                api_key_in: ApiKeyIn::Query,
                ..Default::default()
            },
            extract: vec![crate::model::Extract {
                on: true,
                var: "token".into(),
                from: crate::model::ExtractFrom::JsonPath,
                expr: "$.access_token".into(),
            }],
            transport: crate::model::Transport {
                follow_redirects: false,
                max_redirects: 3,
                compressed: false,
                proxy: "http://proxy:8080".into(),
                ca_cert: "/etc/ca.pem".into(),
                client_cert: String::new(),
                client_key: String::new(),
            },
        }
    }

    #[test]
    fn paths_resolve_side_by_side() {
        let c = default_path();
        let s = session_path();
        assert_eq!(c.parent(), s.parent());
        assert!(c.ends_with("collection.json"));
        assert!(s.ends_with("session.json"));
        eprintln!("collection: {}", c.display());
        eprintln!("session:    {}", s.display());
    }

    #[test]
    fn collection_round_trips_every_field() {
        let path = tmp("coll");
        let coll = Collection {
            requests: vec![loaded_spec(), RequestSpec::default()],
            chains: vec![crate::model::Chain {
                name: "login flow".into(),
                steps: vec![crate::model::ChainStep {
                    on: true,
                    request: "everything".into(),
                    keep_going: false,
                }],
            }],
            variables: vec![KeyVal {
                on: true,
                key: "base".into(),
                value: "https://x.dev".into(),
            }],
        };
        save(&path, &coll).unwrap();
        let back = load(&path);
        assert_eq!(back.requests, coll.requests);
        assert_eq!(back.chains, coll.chains);
        assert_eq!(back.variables, coll.variables);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn session_round_trips_draft() {
        let path = tmp("session").with_file_name("session.json");
        let session = Session {
            draft: loaded_spec(),
            selected: Some(3),
            timeout_secs: 90,
            insecure_tls: true,
            vars: std::collections::BTreeMap::from([("token".to_owned(), "abc".to_owned())]),
        };
        save_session(&path, &session).unwrap();
        let back = load_session(&path).unwrap();
        assert_eq!(back.draft, session.draft);
        assert_eq!(back.selected, Some(3));
        assert_eq!(back.timeout_secs, 90);
        assert!(back.insecure_tls);
        assert_eq!(back.vars.get("token").map(String::as_str), Some("abc"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_or_corrupt_files_fall_back_to_defaults() {
        let missing = tmp("nope");
        assert!(load(&missing).requests.is_empty());
        assert!(load_session(&missing).is_none());

        let bad = tmp("bad");
        std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
        std::fs::write(&bad, "{ not json").unwrap();
        assert!(load(&bad).requests.is_empty());
        assert!(load_session(&bad).is_none());
        let _ = std::fs::remove_dir_all(bad.parent().unwrap());
    }
}
