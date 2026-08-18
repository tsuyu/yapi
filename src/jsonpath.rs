//! A small JSONPath evaluator for pulling values out of a response.
//!
//! Supported: `$`, `.key`, `["key"]`, `[0]`, `[-1]`, `[*]`, `[1:3]`, and `..key`
//! (recursive descent). Filter expressions (`?(@.x > 1)`) are not supported.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
enum Step {
    Key(String),
    Index(i64),
    Wildcard,
    Slice(Option<i64>, Option<i64>),
    /// `..key` - every `key` at any depth.
    Descend(String),
    /// `..*` - every value at any depth.
    DescendAll,
}

/// Evaluate `path` against `value`, returning every match in document order.
pub fn select<'a>(value: &'a Value, path: &str) -> Result<Vec<&'a Value>, String> {
    let steps = parse(path)?;
    let mut current: Vec<&Value> = vec![value];
    for step in &steps {
        let mut next: Vec<&Value> = Vec::new();
        for node in current {
            apply(step, node, &mut next);
        }
        current = next;
    }
    Ok(current)
}

/// Evaluate against raw json text and render the matches as pretty json.
/// A single match is returned bare; several come back as an array.
pub fn select_text(json: &str, path: &str) -> Result<String, String> {
    let value: Value =
        serde_json::from_str(json).map_err(|e| format!("body is not json: {e}"))?;
    let found = select(&value, path)?;
    let rendered = match found.len() {
        0 => return Err("no match".to_owned()),
        1 => serde_json::to_string_pretty(found[0]),
        _ => serde_json::to_string_pretty(&found),
    };
    rendered.map_err(|e| e.to_string())
}

fn apply<'a>(step: &Step, node: &'a Value, out: &mut Vec<&'a Value>) {
    match step {
        Step::Key(key) => {
            if let Some(v) = node.get(key.as_str()) {
                out.push(v);
            }
        }
        Step::Index(i) => {
            if let Some(array) = node.as_array() {
                if let Some(v) = resolve_index(*i, array.len()).and_then(|i| array.get(i)) {
                    out.push(v);
                }
            }
        }
        Step::Wildcard => match node {
            Value::Array(items) => out.extend(items.iter()),
            Value::Object(map) => out.extend(map.values()),
            _ => {}
        },
        Step::Slice(from, to) => {
            if let Some(array) = node.as_array() {
                let len = array.len();
                let start = from
                    .map(|i| resolve_index(i, len).unwrap_or(0))
                    .unwrap_or(0);
                let end = to.map(|i| resolve_index(i, len).unwrap_or(0)).unwrap_or(len);
                if start < end {
                    out.extend(array[start..end.min(len)].iter());
                }
            }
        }
        Step::Descend(key) => descend(node, Some(key), out),
        Step::DescendAll => descend(node, None, out),
    }
}

fn descend<'a>(node: &'a Value, key: Option<&str>, out: &mut Vec<&'a Value>) {
    match node {
        Value::Object(map) => {
            for (k, v) in map {
                match key {
                    Some(want) if k == want => out.push(v),
                    None => out.push(v),
                    _ => {}
                }
                descend(v, key, out);
            }
        }
        Value::Array(items) => {
            for v in items {
                if key.is_none() {
                    out.push(v);
                }
                descend(v, key, out);
            }
        }
        _ => {}
    }
}

/// Negative indices count back from the end, python style.
fn resolve_index(i: i64, len: usize) -> Option<usize> {
    if i >= 0 {
        Some(i as usize)
    } else {
        len.checked_sub(i.unsigned_abs() as usize)
    }
}

fn parse(path: &str) -> Result<Vec<Step>, String> {
    let mut chars = path.trim();
    if chars.is_empty() {
        return Err("empty path".to_owned());
    }
    // "$" is optional: "user.name" reads the same as "$.user.name"
    chars = chars.strip_prefix('$').unwrap_or(chars);

    let mut steps = Vec::new();
    let bytes = chars.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'.' if chars[i..].starts_with("..") => {
                i += 2;
                if chars[i..].starts_with('*') {
                    steps.push(Step::DescendAll);
                    i += 1;
                } else if chars[i..].starts_with('[') {
                    steps.push(Step::DescendAll);
                } else {
                    let name = take_name(&chars[i..]);
                    if name.is_empty() {
                        return Err("`..` needs a key after it".to_owned());
                    }
                    i += name.len();
                    steps.push(Step::Descend(name));
                }
            }
            b'.' => {
                i += 1;
                if chars[i..].starts_with('*') {
                    steps.push(Step::Wildcard);
                    i += 1;
                } else {
                    let name = take_name(&chars[i..]);
                    if name.is_empty() {
                        return Err("expected a key after `.`".to_owned());
                    }
                    i += name.len();
                    steps.push(Step::Key(name));
                }
            }
            b'[' => {
                let end = chars[i..]
                    .find(']')
                    .ok_or_else(|| "unclosed `[`".to_owned())?;
                let inner = chars[i + 1..i + end].trim();
                steps.push(parse_bracket(inner)?);
                i += end + 1;
            }
            _ => {
                // a leading bare key, as in "user.name"
                let name = take_name(&chars[i..]);
                if name.is_empty() {
                    return Err(format!("unexpected {:?} in path", &chars[i..i + 1]));
                }
                i += name.len();
                steps.push(Step::Key(name));
            }
        }
    }
    Ok(steps)
}

fn parse_bracket(inner: &str) -> Result<Step, String> {
    if inner == "*" {
        return Ok(Step::Wildcard);
    }
    if let Some(quoted) = inner
        .strip_prefix('\'')
        .and_then(|r| r.strip_suffix('\''))
        .or_else(|| inner.strip_prefix('"').and_then(|r| r.strip_suffix('"')))
    {
        return Ok(Step::Key(quoted.to_owned()));
    }
    if let Some((from, to)) = inner.split_once(':') {
        let parse_end = |s: &str| -> Result<Option<i64>, String> {
            match s.trim() {
                "" => Ok(None),
                n => n
                    .parse::<i64>()
                    .map(Some)
                    .map_err(|_| format!("bad slice bound {n:?}")),
            }
        };
        return Ok(Step::Slice(parse_end(from)?, parse_end(to)?));
    }
    inner
        .parse::<i64>()
        .map(Step::Index)
        .map_err(|_| format!("bad index {inner:?} - quote it for a key"))
}

fn take_name(rest: &str) -> String {
    rest.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> Value {
        serde_json::json!({
            "store": {
                "name": "corner shop",
                "books": [
                    {"title": "a", "price": 10, "tags": ["x", "y"]},
                    {"title": "b", "price": 20},
                    {"title": "c", "price": 30}
                ]
            },
            "ok": true
        })
    }

    fn titles(path: &str) -> Vec<String> {
        select(&doc(), path)
            .unwrap()
            .iter()
            .map(|v| v.to_string())
            .collect()
    }

    #[test]
    fn walks_keys_and_indices() {
        assert_eq!(titles("$.store.name"), vec!["\"corner shop\""]);
        assert_eq!(titles("$.store.books[0].title"), vec!["\"a\""]);
        assert_eq!(titles("store.books[1].price"), vec!["20"]);
        assert_eq!(titles(r#"$["store"]["name"]"#), vec!["\"corner shop\""]);
        assert_eq!(titles("$.store.books[-1].title"), vec!["\"c\""]);
    }

    #[test]
    fn wildcards_and_slices() {
        assert_eq!(titles("$.store.books[*].title").len(), 3);
        assert_eq!(titles("$.store.books[0:2].title"), vec!["\"a\"", "\"b\""]);
        assert_eq!(titles("$.store.books[1:].price"), vec!["20", "30"]);
    }

    #[test]
    fn recursive_descent() {
        assert_eq!(titles("$..title"), vec!["\"a\"", "\"b\"", "\"c\""]);
        assert_eq!(titles("$..price").len(), 3);
        // nested arrays are reached too
        assert_eq!(titles("$..tags[0]"), vec!["\"x\""]);
    }

    #[test]
    fn misses_are_empty_not_errors() {
        assert!(select(&doc(), "$.nope").unwrap().is_empty());
        assert!(select(&doc(), "$.store.books[99]").unwrap().is_empty());
        assert_eq!(select_text(&doc().to_string(), "$.nope"), Err("no match".to_owned()));
    }

    #[test]
    fn bad_paths_explain_themselves() {
        assert!(parse("$.").is_err());
        assert!(parse("$[1").is_err());
        assert!(parse("$[abc]").unwrap_err().contains("quote it"));
        assert!(parse("$..").is_err());
    }

    #[test]
    fn renders_one_match_bare_and_many_as_an_array() {
        let json = doc().to_string();
        assert_eq!(select_text(&json, "$.ok").unwrap(), "true");
        let many = select_text(&json, "$..title").unwrap();
        assert!(many.starts_with('['), "{many}");
        assert!(many.contains("\"a\""));
    }
}
