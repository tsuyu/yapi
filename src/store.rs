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
    config_dir_for("yAPI")
        .map(|d| d.join("collection.json"))
        .unwrap_or_else(|| PathBuf::from("collection.json"))
}

pub fn session_path() -> PathBuf {
    default_path().with_file_name("session.json")
}

fn config_dir_for(app: &str) -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", app).map(|d| d.config_dir().to_path_buf())
}

/// The name this app shipped under before it was renamed to yAPI. Collections
/// written by that build are still on disk under the old directory.
const LEGACY_APP_NAME: &str = "api-req";

/// Copy `collection.json` / `session.json` from the pre-rename config directory
/// for whichever of them this one does not have yet.
///
/// Each file is considered on its own. Guarding on "the new directory is empty"
/// instead would strand a collection as soon as anything else wrote there
/// first - a single launch of the renamed build autosaves `session.json`, and
/// that alone would have been enough to lose the collection forever.
///
/// Returns the files it copied, for a toast on first launch.
pub fn migrate_legacy_config() -> Vec<String> {
    let (Some(new_dir), Some(old_dir)) = (config_dir_for("yAPI"), config_dir_for(LEGACY_APP_NAME))
    else {
        return Vec::new();
    };
    migrate_between(&old_dir, &new_dir)
}

/// The copying itself, with the directories handed in so it can be tested
/// without touching the real config location.
fn migrate_between(old_dir: &Path, new_dir: &Path) -> Vec<String> {
    if old_dir == new_dir || !old_dir.is_dir() {
        return Vec::new();
    }

    let names = ["collection.json", "session.json"];
    let wanted: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| old_dir.join(n).is_file() && !new_dir.join(n).exists())
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    if std::fs::create_dir_all(new_dir).is_err() {
        return Vec::new();
    }

    let mut copied = Vec::new();
    for name in wanted {
        if std::fs::copy(old_dir.join(name), new_dir.join(name)).is_ok() {
            copied.push(name.to_owned());
        }
    }
    copied
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
            assertions: vec![crate::model::Assertion {
                on: true,
                from: crate::model::AssertOn::JsonPath,
                expr: "$.id".into(),
                op: crate::model::AssertOp::Exists,
                value: String::new(),
            }],
            graphql_vars: "{\"id\": 1}".to_owned(),
            folder: "admin".to_owned(),
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

    /// Two empty directories to stand in for the old and new config locations.
    fn migration_dirs(name: &str) -> (PathBuf, PathBuf) {
        let mut base = std::env::temp_dir();
        base.push(format!("yapi-migrate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let old = base.join("api-req");
        let new = base.join("yAPI");
        std::fs::create_dir_all(&old).unwrap();
        (old, new)
    }

    #[test]
    fn a_collection_under_the_old_name_is_brought_across() {
        let (old, new) = migration_dirs("both");
        std::fs::write(old.join("collection.json"), r#"{"requests":[]}"#).unwrap();
        std::fs::write(old.join("session.json"), r#"{"selected":3}"#).unwrap();

        let copied = migrate_between(&old, &new);
        assert_eq!(copied, vec!["collection.json", "session.json"]);
        assert_eq!(
            std::fs::read_to_string(new.join("collection.json")).unwrap(),
            r#"{"requests":[]}"#
        );
        // one shot: a second call has nothing left to do
        assert!(migrate_between(&old, &new).is_empty());

        let _ = std::fs::remove_dir_all(old.parent().unwrap());
    }

    #[test]
    fn a_session_written_first_does_not_strand_the_collection() {
        // this is the real case: one launch of the renamed build autosaves a
        // session before any migration runs. The collection must still arrive.
        let (old, new) = migration_dirs("partial");
        std::fs::write(old.join("collection.json"), r#"{"requests":[1]}"#).unwrap();
        std::fs::write(old.join("session.json"), "old-session").unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("session.json"), "new-session").unwrap();

        let copied = migrate_between(&old, &new);
        assert_eq!(copied, vec!["collection.json"]);
        // and the session already in place is left exactly as it was
        assert_eq!(
            std::fs::read_to_string(new.join("session.json")).unwrap(),
            "new-session"
        );

        let _ = std::fs::remove_dir_all(old.parent().unwrap());
    }

    #[test]
    fn nothing_to_migrate_is_not_an_error() {
        let (old, new) = migration_dirs("empty");
        // old directory exists but holds nothing
        assert!(migrate_between(&old, &new).is_empty());
        assert!(!new.exists(), "an empty migration should not create the directory");

        // and a missing old directory
        let missing = old.join("nope");
        assert!(migrate_between(&missing, &new).is_empty());
        // and the same directory twice
        assert!(migrate_between(&old, &old).is_empty());

        let _ = std::fs::remove_dir_all(old.parent().unwrap());
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
            environments: vec![crate::model::Environment {
                name: "prod".into(),
                vars: vec![KeyVal {
                    on: true,
                    key: "base".into(),
                    value: "https://api.live".into(),
                }],
            }],
            active_env: Some(0),
            folders: vec!["admin".into(), "public".into()],
        };
        save(&path, &coll).unwrap();
        let back = load(&path);
        assert_eq!(back.requests, coll.requests);
        assert_eq!(back.chains, coll.chains);
        assert_eq!(back.variables, coll.variables);
        assert_eq!(back.environments, coll.environments);
        assert_eq!(back.active_env, Some(0));
        assert_eq!(back.folders, coll.folders);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_active_environment_wins_over_the_collection_defaults() {
        let coll = Collection {
            variables: vec![
                KeyVal { on: true, key: "base".into(), value: "https://dev.local".into() },
                KeyVal { on: true, key: "shared".into(), value: "same".into() },
            ],
            environments: vec![crate::model::Environment {
                name: "prod".into(),
                vars: vec![KeyVal {
                    on: true,
                    key: "base".into(),
                    value: "https://api.live".into(),
                }],
            }],
            active_env: Some(0),
            ..Default::default()
        };
        let vars = coll.resolved_vars();
        assert_eq!(vars.get("base").map(String::as_str), Some("https://api.live"));
        // a name the environment does not mention falls through to the default
        assert_eq!(vars.get("shared").map(String::as_str), Some("same"));

        let base_only = Collection { active_env: None, ..coll };
        assert_eq!(
            base_only.resolved_vars().get("base").map(String::as_str),
            Some("https://dev.local")
        );
    }

    #[test]
    fn an_export_without_secrets_keeps_everything_that_is_not_one() {
        let mut spec = loaded_spec();
        spec.auth = crate::model::Auth {
            kind: crate::model::AuthKind::OAuth2,
            token: "bearer-tok".into(),
            password: "hunter2".into(),
            api_key_value: "k3y".into(),
            oauth: crate::model::OAuth2 {
                client_id: "public-id".into(),
                client_secret: "shhh".into(),
                token_url: "https://auth.example.com/token".into(),
                access_token: "at".into(),
                refresh_token: "rt".into(),
                expires_at: 99,
                ..Default::default()
            },
            ..Default::default()
        };
        let coll = Collection {
            requests: vec![spec],
            variables: vec![KeyVal { on: true, key: "tok".into(), value: "secret".into() }],
            ..Default::default()
        };

        assert!(!coll.secrets_summary().is_empty());
        let clean = coll.scrubbed();
        let auth = &clean.requests[0].auth;
        assert!(auth.token.is_empty());
        assert!(auth.password.is_empty());
        assert!(auth.api_key_value.is_empty());
        assert!(auth.oauth.client_secret.is_empty());
        assert!(auth.oauth.access_token.is_empty());
        assert!(auth.oauth.refresh_token.is_empty());
        assert_eq!(auth.oauth.expires_at, 0);
        assert!(clean.variables[0].value.is_empty());

        // the parts worth sharing survive
        assert_eq!(auth.kind, crate::model::AuthKind::OAuth2);
        assert_eq!(auth.oauth.client_id, "public-id");
        assert_eq!(auth.oauth.token_url, "https://auth.example.com/token");
        assert_eq!(clean.requests[0].url, coll.requests[0].url);
        assert_eq!(clean.variables[0].key, "tok");
        assert!(clean.secrets_summary().is_empty());
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

