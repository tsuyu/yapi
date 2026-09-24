//! Check a response against what it was supposed to be. Same shape as
//! `extract`: rules live on the request, run after every send, and report
//! themselves one by one rather than stopping at the first problem.
//!
//! A failing assertion is a *test* failure, not a transport failure - the
//! response is still shown, it is just marked wrong.

use crate::model::{AssertOn, AssertOp, Assertion};
use crate::net::ResponseData;

pub struct Check {
    pub label: String,
    pub passed: bool,
    /// What was actually there, or why the check could not run.
    pub detail: String,
}

pub struct Checks {
    pub results: Vec<Check>,
}

impl Checks {
    pub fn passed(&self) -> usize {
        self.results.iter().filter(|c| c.passed).count()
    }

    /// One line for the toast and the chain log; `None` when nothing was checked.
    pub fn summary(&self) -> Option<String> {
        if self.results.is_empty() {
            return None;
        }
        let passed = self.passed();
        let total = self.results.len();
        if passed == total {
            return Some(format!("{passed}/{total} checks passed"));
        }
        let failed: Vec<&str> = self
            .results
            .iter()
            .filter(|c| !c.passed)
            .map(|c| c.label.as_str())
            .collect();
        Some(format!(
            "{passed}/{total} checks passed - failed: {}",
            failed.join("; ")
        ))
    }
}

/// Run every active assertion against a response.
pub fn apply(rules: &[Assertion], resp: &ResponseData) -> Checks {
    let results = rules
        .iter()
        .filter(|r| r.active())
        .map(|rule| {
            let label = rule.label();
            match one(rule, resp) {
                Ok((passed, detail)) => Check {
                    label,
                    passed,
                    detail,
                },
                // a rule that cannot run is a failure, not a silent skip: a
                // typo'd jsonpath should not read as a green tick
                Err(e) => Check {
                    label,
                    passed: false,
                    detail: e,
                },
            }
        })
        .collect();
    Checks { results }
}

/// `Ok((passed, detail))`, or `Err` when the rule itself is unusable.
fn one(rule: &Assertion, resp: &ResponseData) -> Result<(bool, String), String> {
    let expr = rule.expr.trim();
    if rule.from.needs_expr() && expr.is_empty() {
        return Err(format!("{} needs an expression", rule.from.as_str()));
    }

    // `exists` / `is absent` ask only whether the value is there at all, so they
    // are answered before anything tries to read it
    let found = lookup(rule.from, expr, resp)?;
    match rule.op {
        AssertOp::Exists => {
            return Ok(match &found {
                Some(v) => (true, format!("found {}", short(v))),
                None => (false, "not present".to_owned()),
            })
        }
        AssertOp::Missing => {
            return Ok(match &found {
                Some(v) => (false, format!("present: {}", short(v))),
                None => (true, "absent".to_owned()),
            })
        }
        _ => {}
    }

    let Some(actual) = found else {
        return Ok((false, "not present in the response".to_owned()));
    };
    let want = rule.value.trim();

    let passed = match rule.op {
        AssertOp::Eq => actual.trim() == want,
        AssertOp::Ne => actual.trim() != want,
        AssertOp::Contains => actual.contains(want),
        AssertOp::NotContains => !actual.contains(want),
        AssertOp::Matches => regex::Regex::new(want)
            .map_err(|e| format!("bad regex: {e}"))?
            .is_match(&actual),
        AssertOp::Lt | AssertOp::Gt => {
            let a: f64 = actual
                .trim()
                .parse()
                .map_err(|_| format!("{} is not a number", short(&actual)))?;
            let b: f64 = want
                .parse()
                .map_err(|_| format!("{want} is not a number"))?;
            if rule.op == AssertOp::Lt {
                a < b
            } else {
                a > b
            }
        }
        AssertOp::Exists | AssertOp::Missing => unreachable!("handled above"),
    };
    Ok((passed, format!("got {}", short(&actual))))
}

/// The value this rule points at, or `None` when the response does not have it.
fn lookup(from: AssertOn, expr: &str, resp: &ResponseData) -> Result<Option<String>, String> {
    Ok(match from {
        AssertOn::Status => Some(resp.status.to_string()),
        AssertOn::ElapsedMs => Some(resp.elapsed_ms.to_string()),
        AssertOn::Body => Some(resp.body.clone()),
        AssertOn::Header => resp
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(expr))
            .map(|(_, v)| v.clone()),
        AssertOn::Cookie => resp
            .cookies()
            .into_iter()
            .find(|(name, _, _)| name.eq_ignore_ascii_case(expr))
            .map(|(_, value, _)| value),
        AssertOn::JsonPath => {
            let value: serde_json::Value = serde_json::from_str(&resp.body)
                .map_err(|e| format!("response is not json: {e}"))?;
            match crate::jsonpath::select(&value, expr)?.first() {
                None => None,
                // match extract: a json string compares as the bare string
                Some(serde_json::Value::String(s)) => Some(s.clone()),
                Some(other) => Some(other.to_string()),
            }
        }
    })
}

/// Keep a whole response body out of a one-line result.
fn short(text: &str) -> String {
    let one_line = text.replace(['\n', '\r'], " ");
    let trimmed = one_line.trim();
    if trimmed.chars().count() <= 60 {
        return trimmed.to_owned();
    }
    let head: String = trimmed.chars().take(57).collect();
    format!("{head}...")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Shape;

    fn response(status: u16, body: &str, headers: Vec<(&str, &str)>) -> ResponseData {
        ResponseData {
            status,
            status_text: "OK".into(),
            elapsed_ms: 42,
            size: body.len(),
            headers: headers
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            body: body.to_owned(),
            pretty: None,
            final_url: "http://x.dev/thing".into(),
            content_type: "application/json".into(),
            shape: Shape::Json,
            bytes: body.as_bytes().to_vec(),
        }
    }

    fn rule(from: AssertOn, expr: &str, op: AssertOp, value: &str) -> Assertion {
        Assertion {
            on: true,
            from,
            expr: expr.to_owned(),
            op,
            value: value.to_owned(),
        }
    }

    #[test]
    fn a_passing_suite_reports_every_check() {
        let resp = response(
            200,
            r#"{"id":7,"name":"ada","tags":["x"]}"#,
            vec![("Content-Type", "application/json")],
        );
        let out = apply(
            &[
                rule(AssertOn::Status, "", AssertOp::Eq, "200"),
                rule(AssertOn::JsonPath, "$.name", AssertOp::Eq, "ada"),
                rule(AssertOn::JsonPath, "$.id", AssertOp::Gt, "3"),
                rule(
                    AssertOn::Header,
                    "content-type",
                    AssertOp::Contains,
                    "json",
                ),
                rule(AssertOn::ElapsedMs, "", AssertOp::Lt, "1000"),
            ],
            &resp,
        );
        assert_eq!(out.passed(), out.results.len(), "{:?}", out.summary());
        assert_eq!(out.summary().unwrap(), "5/5 checks passed");
    }

    #[test]
    fn failures_name_themselves_and_show_what_was_there() {
        let resp = response(500, r#"{"error":"boom"}"#, vec![]);
        let out = apply(
            &[
                rule(AssertOn::Status, "", AssertOp::Eq, "200"),
                rule(AssertOn::JsonPath, "$.error", AssertOp::Eq, "boom"),
            ],
            &resp,
        );
        assert_eq!(out.passed(), 1);
        assert!(!out.results[0].passed);
        assert_eq!(out.results[0].label, "status == 200");
        assert_eq!(out.results[0].detail, "got 500");
        let summary = out.summary().unwrap();
        assert!(summary.contains("1/2"), "{summary}");
        assert!(summary.contains("status == 200"), "{summary}");
    }

    #[test]
    fn exists_and_absent_ignore_the_value_box() {
        let resp = response(
            200,
            r#"{"token":"t"}"#,
            vec![("set-cookie", "session=s3cr3t; HttpOnly")],
        );
        let out = apply(
            &[
                rule(AssertOn::JsonPath, "$.token", AssertOp::Exists, ""),
                rule(AssertOn::JsonPath, "$.nope", AssertOp::Missing, ""),
                rule(AssertOn::Cookie, "session", AssertOp::Exists, ""),
                rule(AssertOn::Header, "X-Gone", AssertOp::Missing, ""),
                // the ones that should fail
                rule(AssertOn::JsonPath, "$.nope", AssertOp::Exists, ""),
                rule(AssertOn::JsonPath, "$.token", AssertOp::Missing, ""),
            ],
            &resp,
        );
        assert_eq!(out.passed(), 4);
        assert!(!out.results[4].passed);
        assert!(!out.results[5].passed);
    }

    #[test]
    fn a_rule_that_cannot_run_fails_rather_than_passing_quietly() {
        let resp = response(200, "not json at all", vec![]);
        let out = apply(
            &[
                rule(AssertOn::JsonPath, "$.a", AssertOp::Eq, "1"),
                rule(AssertOn::Body, "", AssertOp::Matches, "([unclosed"),
                rule(AssertOn::Status, "", AssertOp::Lt, "not-a-number"),
            ],
            &resp,
        );
        assert_eq!(out.passed(), 0);
        assert!(out.results[0].detail.contains("not json"));
        assert!(out.results[1].detail.contains("bad regex"));
        assert!(out.results[2].detail.contains("not a number"));
    }

    #[test]
    fn matches_and_contains_work_on_the_raw_body() {
        let resp = response(201, r#"{"id":"user_8812"}"#, vec![]);
        let out = apply(
            &[
                rule(AssertOn::Body, "", AssertOp::Contains, "user_"),
                rule(AssertOn::Body, "", AssertOp::NotContains, "error"),
                rule(AssertOn::JsonPath, "$.id", AssertOp::Matches, r"^user_\d+$"),
            ],
            &resp,
        );
        assert_eq!(out.passed(), 3);
    }

    #[test]
    fn inactive_and_unnamed_rules_are_skipped() {
        let resp = response(200, "{}", vec![]);
        let mut off = rule(AssertOn::Status, "", AssertOp::Eq, "200");
        off.on = false;
        let no_expr = rule(AssertOn::Header, "  ", AssertOp::Exists, "");
        let out = apply(&[off, no_expr], &resp);
        assert!(out.results.is_empty());
        assert!(out.summary().is_none());
    }

    #[test]
    fn long_values_are_truncated_in_the_detail_line() {
        let body = format!(r#"{{"blob":"{}"}}"#, "x".repeat(500));
        let resp = response(200, &body, vec![]);
        let out = apply(
            &[rule(AssertOn::JsonPath, "$.blob", AssertOp::Eq, "short")],
            &resp,
        );
        assert!(out.results[0].detail.len() < 80, "{}", out.results[0].detail);
        assert!(out.results[0].detail.ends_with("..."));
    }
}
