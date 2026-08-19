//! Run a sequence of requests, letting extracted values flow into the ones
//! that follow.

use std::collections::BTreeMap;
use std::sync::mpsc::Sender;

use crate::model::RequestSpec;
use crate::net::SendOpts;

/// What happened to one step, as it happens.
pub enum ChainMsg {
    Started {
        index: usize,
        name: String,
    },
    Done {
        index: usize,
        name: String,
        status: u16,
        elapsed_ms: u128,
        /// Variables this step produced.
        extracted: Vec<(String, String)>,
        /// Rules that could not be applied.
        warnings: Vec<String>,
    },
    Failed {
        index: usize,
        name: String,
        error: String,
    },
    Finished {
        ran: usize,
        ok: bool,
    },
}

pub struct Step {
    pub spec: RequestSpec,
    pub keep_going: bool,
}

/// Run the steps on a worker thread, reporting progress on `tx`.
pub fn spawn_run(
    steps: Vec<Step>,
    vars: BTreeMap<String, String>,
    opts: SendOpts,
    tx: Sender<ChainMsg>,
    ctx: egui::Context,
) {
    std::thread::spawn(move || {
        let mut vars = vars;
        let mut ok = true;
        let mut ran = 0;

        for (index, step) in steps.iter().enumerate() {
            let name = step.spec.name.clone();
            let _ = tx.send(ChainMsg::Started {
                index,
                name: name.clone(),
            });
            ctx.request_repaint();

            // resolve against everything learned so far, then send
            let resolved = step.spec.resolve(&vars);
            let missing = resolved.missing_vars(&vars);
            if !missing.is_empty() {
                let _ = tx.send(ChainMsg::Failed {
                    index,
                    name,
                    error: format!("no value for {}", missing.join(", ")),
                });
                ran += 1;
                ok = false;
                if step.keep_going {
                    continue;
                }
                break;
            }

            ran += 1;
            match crate::net::run_blocking(resolved.clone(), opts) {
                Ok(resp) => {
                    let out = crate::extract::apply(&resolved.extract, &resp);
                    for (k, v) in &out.values {
                        vars.insert(k.clone(), v.clone());
                    }
                    let failed_status = resp.status >= 400;
                    let _ = tx.send(ChainMsg::Done {
                        index,
                        name,
                        status: resp.status,
                        elapsed_ms: resp.elapsed_ms,
                        extracted: out.values,
                        warnings: out.errors,
                    });
                    if failed_status {
                        ok = false;
                        if !step.keep_going {
                            ctx.request_repaint();
                            break;
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(ChainMsg::Failed {
                        index,
                        name,
                        error: e,
                    });
                    ok = false;
                    if !step.keep_going {
                        ctx.request_repaint();
                        break;
                    }
                }
            }
            ctx.request_repaint();
        }

        let _ = tx.send(ChainMsg::Finished { ran, ok });
        ctx.request_repaint();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Extract, ExtractFrom, KeyVal, Method};
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    /// A server that answers a fixed number of requests, recording each one and
    /// replying with the matching canned body.
    fn scripted(replies: Vec<&'static str>) -> (u16, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for body in replies {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(&stream);
                let mut request = String::new();
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
                    request.push_str(&line);
                    if end {
                        break;
                    }
                }
                if len > 0 {
                    let mut buf = vec![0u8; len];
                    std::io::Read::read_exact(&mut reader, &mut buf).unwrap();
                    request.push_str(&String::from_utf8_lossy(&buf));
                }
                seen.push(request);

                let mut out = &stream;
                write!(
                    out,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                out.write_all(body.as_bytes()).unwrap();
                out.flush().unwrap();
            }
            seen
        });
        (port, handle)
    }

    fn step(spec: RequestSpec) -> Step {
        Step {
            spec,
            keep_going: false,
        }
    }

    fn collect(rx: std::sync::mpsc::Receiver<ChainMsg>) -> (Vec<String>, bool, usize) {
        let mut log = Vec::new();
        let mut ok = false;
        let mut ran = 0;
        while let Ok(msg) = rx.recv() {
            match msg {
                ChainMsg::Started { .. } => {}
                ChainMsg::Done {
                    name,
                    status,
                    extracted,
                    warnings,
                    ..
                } => {
                    log.push(format!(
                        "{name} {status} {:?} warn={}",
                        extracted,
                        warnings.len()
                    ));
                }
                ChainMsg::Failed { name, error, .. } => log.push(format!("{name} FAILED {error}")),
                ChainMsg::Finished { ran: r, ok: o } => {
                    ran = r;
                    ok = o;
                    break;
                }
            }
        }
        (log, ok, ran)
    }

    #[test]
    fn login_then_me_then_user_by_id() {
        let (port, server) = scripted(vec![
            r#"{"access_token":"tok-1"}"#,
            r#"{"id":42,"name":"dev"}"#,
            r#"{"id":42,"email":"dev@x.dev"}"#,
        ]);

        let login = RequestSpec {
            name: "login".into(),
            method: Method::Post,
            url: format!("localhost:{port}/login"),
            params: vec![],
            headers: vec![],
            body_kind: crate::model::BodyKind::Json,
            body: r#"{"user":"dev"}"#.into(),
            extract: vec![Extract {
                on: true,
                var: "access_token".into(),
                from: ExtractFrom::JsonPath,
                expr: "$.access_token".into(),
            }],
            ..Default::default()
        };
        let me = RequestSpec {
            name: "me".into(),
            url: format!("localhost:{port}/users/me"),
            params: vec![],
            headers: vec![KeyVal {
                on: true,
                key: "Authorization".into(),
                value: "Bearer {{access_token}}".into(),
            }],
            extract: vec![Extract {
                on: true,
                var: "user_id".into(),
                from: ExtractFrom::JsonPath,
                expr: "$.id".into(),
            }],
            ..Default::default()
        };
        let by_id = RequestSpec {
            name: "user by id".into(),
            url: format!("localhost:{port}/users/{{{{user_id}}}}"),
            params: vec![],
            headers: vec![],
            extract: vec![],
            ..Default::default()
        };

        let (tx, rx) = std::sync::mpsc::channel();
        spawn_run(
            vec![step(login), step(me), step(by_id)],
            BTreeMap::new(),
            SendOpts {
                timeout_secs: 10,
                insecure_tls: false,
            },
            tx,
            egui::Context::default(),
        );
        let (log, ok, ran) = collect(rx);

        assert!(ok, "chain should succeed: {log:?}");
        assert_eq!(ran, 3);
        assert!(log[0].contains("access_token"), "{log:?}");
        assert!(log[1].contains("user_id"), "{log:?}");

        let seen = server.join().unwrap();
        assert!(seen[0].starts_with("POST /login "), "{}", seen[0]);
        // the token from step 1 reached step 2's header
        assert!(
            seen[1].contains("authorization: Bearer tok-1")
                || seen[1].contains("Authorization: Bearer tok-1"),
            "token did not flow:\n{}",
            seen[1]
        );
        // the id from step 2 reached step 3's url
        assert!(seen[2].starts_with("GET /users/42 "), "{}", seen[2]);
    }

    #[test]
    fn a_failed_step_stops_the_chain() {
        // the server only answers once, so step two hits a closed port
        let (port, _server) = scripted(vec![r#"{"ok":true}"#]);
        let first = RequestSpec {
            name: "one".into(),
            url: format!("localhost:{port}/a"),
            params: vec![],
            headers: vec![],
            extract: vec![],
            ..Default::default()
        };
        let second = RequestSpec {
            name: "two".into(),
            url: format!("localhost:{port}/b"),
            params: vec![],
            headers: vec![],
            extract: vec![],
            ..Default::default()
        };
        let (tx, rx) = std::sync::mpsc::channel();
        spawn_run(
            vec![step(first), step(second)],
            BTreeMap::new(),
            SendOpts {
                timeout_secs: 5,
                insecure_tls: false,
            },
            tx,
            egui::Context::default(),
        );
        let (log, ok, ran) = collect(rx);
        assert!(!ok, "{log:?}");
        assert_eq!(ran, 2);
        assert!(log[1].contains("FAILED"), "{log:?}");
    }

    #[test]
    fn a_missing_variable_stops_before_sending() {
        let spec = RequestSpec {
            name: "needs a var".into(),
            url: "http://127.0.0.1:1/{{never_set}}".into(),
            params: vec![],
            headers: vec![],
            extract: vec![],
            ..Default::default()
        };
        let (tx, rx) = std::sync::mpsc::channel();
        spawn_run(
            vec![step(spec)],
            BTreeMap::new(),
            SendOpts {
                timeout_secs: 5,
                insecure_tls: false,
            },
            tx,
            egui::Context::default(),
        );
        let (log, ok, _) = collect(rx);
        assert!(!ok);
        assert!(log[0].contains("no value for never_set"), "{log:?}");
    }
}
