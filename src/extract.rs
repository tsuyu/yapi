//! Pull values out of a response and into variables, so the next request in a
//! chain can use them as `{{name}}`.

use crate::model::{Extract, ExtractFrom};
use crate::net::ResponseData;

pub struct Extracted {
    pub values: Vec<(String, String)>,
    pub errors: Vec<String>,
}

/// Run every active rule against a response.
pub fn apply(rules: &[Extract], resp: &ResponseData) -> Extracted {
    let mut values = Vec::new();
    let mut errors = Vec::new();

    for rule in rules.iter().filter(|r| r.active()) {
        let var = rule.var.trim().to_owned();
        match one(rule, resp) {
            Ok(value) => values.push((var, value)),
            Err(e) => errors.push(format!("{var}: {e}")),
        }
    }
    Extracted { values, errors }
}

fn one(rule: &Extract, resp: &ResponseData) -> Result<String, String> {
    let expr = rule.expr.trim();
    if rule.from.needs_expr() && expr.is_empty() {
        return Err(format!("{} needs an expression", rule.from.as_str()));
    }

    match rule.from {
        ExtractFrom::Status => Ok(resp.status.to_string()),
        ExtractFrom::Body => Ok(resp.body.clone()),
        ExtractFrom::Header => resp
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(expr))
            .map(|(_, v)| v.clone())
            .ok_or_else(|| format!("no {expr} header in the response")),
        ExtractFrom::Cookie => resp
            .cookies()
            .into_iter()
            .find(|(name, _, _)| name.eq_ignore_ascii_case(expr))
            .map(|(_, value, _)| value)
            .ok_or_else(|| format!("no {expr} cookie in the response")),
        ExtractFrom::JsonPath => {
            let value: serde_json::Value = serde_json::from_str(&resp.body)
                .map_err(|e| format!("response is not json: {e}"))?;
            let found = crate::jsonpath::select(&value, expr)?;
            match found.first() {
                None => Err(format!("{expr} matched nothing")),
                // a json string becomes the bare string, not a quoted one
                Some(serde_json::Value::String(s)) => Ok(s.clone()),
                Some(other) => Ok(other.to_string()),
            }
        }
        ExtractFrom::Regex => {
            let re = regex::Regex::new(expr).map_err(|e| format!("bad regex: {e}"))?;
            let caps = re
                .captures(&resp.body)
                .ok_or_else(|| "regex matched nothing".to_owned())?;
            // group 1 when the pattern has one, else the whole match
            let m = caps
                .get(1)
                .or_else(|| caps.get(0))
                .ok_or_else(|| "regex matched nothing".to_owned())?;
            Ok(m.as_str().to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::Shape;

    fn response(body: &str, headers: Vec<(&str, &str)>) -> ResponseData {
        ResponseData {
            status: 200,
            status_text: "OK".into(),
            elapsed_ms: 3,
            size: body.len(),
            headers: headers
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            body: body.to_owned(),
            pretty: None,
            final_url: "http://x.dev/login".into(),
            content_type: "application/json".into(),
            shape: Shape::Json,
            bytes: body.as_bytes().to_vec(),
        }
    }

    fn rule(var: &str, from: ExtractFrom, expr: &str) -> Extract {
        Extract {
            on: true,
            var: var.to_owned(),
            from,
            expr: expr.to_owned(),
        }
    }

    #[test]
    fn login_response_yields_a_token() {
        let resp = response(
            r#"{"access_token":"abc.def","expires_in":3600,"user":{"id":42}}"#,
            vec![],
        );
        let out = apply(
            &[
                rule("token", ExtractFrom::JsonPath, "$.access_token"),
                rule("user_id", ExtractFrom::JsonPath, "$.user.id"),
                rule("code", ExtractFrom::Status, ""),
            ],
            &resp,
        );
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(
            out.values,
            vec![
                ("token".to_owned(), "abc.def".to_owned()),
                ("user_id".to_owned(), "42".to_owned()),
                ("code".to_owned(), "200".to_owned()),
            ]
        );
    }

    #[test]
    fn headers_and_cookies() {
        let resp = response(
            "{}",
            vec![
                ("Location", "/users/7"),
                ("set-cookie", "session=s3cr3t; HttpOnly"),
            ],
        );
        let out = apply(
            &[
                rule("loc", ExtractFrom::Header, "location"),
                rule("sid", ExtractFrom::Cookie, "session"),
            ],
            &resp,
        );
        assert!(out.errors.is_empty());
        assert_eq!(out.values[0].1, "/users/7");
        assert_eq!(out.values[1].1, "s3cr3t");
    }

    #[test]
    fn regex_prefers_the_first_capture_group() {
        let resp = response(r#"<input name="csrf" value="tok-99">"#, vec![]);
        let out = apply(&[rule("csrf", ExtractFrom::Regex, r#"value="([^"]+)""#)], &resp);
        assert_eq!(out.values[0].1, "tok-99");

        // no group: the whole match
        let out = apply(&[rule("m", ExtractFrom::Regex, "tok-[0-9]+")], &resp);
        assert_eq!(out.values[0].1, "tok-99");
    }

    #[test]
    fn failures_are_reported_per_rule_not_fatal() {
        let resp = response(r#"{"a":1}"#, vec![]);
        let out = apply(
            &[
                rule("good", ExtractFrom::JsonPath, "$.a"),
                rule("missing", ExtractFrom::JsonPath, "$.nope"),
                rule("nohdr", ExtractFrom::Header, "X-Nope"),
                rule("bad_re", ExtractFrom::Regex, "([unclosed"),
                rule("blank", ExtractFrom::Header, ""),
            ],
            &resp,
        );
        assert_eq!(out.values, vec![("good".to_owned(), "1".to_owned())]);
        assert_eq!(out.errors.len(), 4);
        assert!(out.errors[0].starts_with("missing:"));
        assert!(out.errors[3].contains("needs an expression"));
    }

    #[test]
    fn inactive_rules_are_skipped() {
        let resp = response(r#"{"a":1}"#, vec![]);
        let mut off = rule("x", ExtractFrom::JsonPath, "$.a");
        off.on = false;
        let unnamed = rule("  ", ExtractFrom::JsonPath, "$.a");
        let out = apply(&[off, unnamed], &resp);
        assert!(out.values.is_empty());
        assert!(out.errors.is_empty());
    }
}
