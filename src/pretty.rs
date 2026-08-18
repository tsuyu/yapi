//! Formatting helpers for the response viewer: xml/html indenting, tag
//! stripping, byte sizes, and a hex dump for bodies that are not text.

/// Re-indent xml or html. This is a formatter, not a parser: it keeps the
/// document's own text, it only decides where the newlines go.
pub fn xml(source: &str) -> String {
    const VOID: [&str; 14] = [
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "track", "wbr",
    ];

    let mut out = String::with_capacity(source.len() + source.len() / 4);
    let mut depth: usize = 0;
    let mut rest = source.trim();

    while !rest.is_empty() {
        let Some(open) = rest.find('<') else {
            push_text(&mut out, rest, depth);
            break;
        };
        // text sitting before this tag
        push_text(&mut out, &rest[..open], depth);
        rest = &rest[open..];

        // <script> and <style> hold code, not markup - copy them verbatim
        if let Some(raw_end) = raw_block_end(rest) {
            let (block, tail) = rest.split_at(raw_end);
            push_line(&mut out, block.trim_end(), depth);
            rest = tail;
            continue;
        }

        let Some(close) = rest.find('>') else {
            push_text(&mut out, rest, depth);
            break;
        };
        let tag = &rest[..=close];
        rest = &rest[close + 1..];

        let inner = tag.trim_start_matches('<').trim_end_matches('>');
        let is_close = inner.starts_with('/');
        let is_self_closing = inner.ends_with('/')
            || inner.starts_with('?')
            || inner.starts_with('!')
            || VOID.contains(&tag_name(inner).as_str());

        if is_close {
            depth = depth.saturating_sub(1);
        }
        push_line(&mut out, tag, depth);
        if !is_close && !is_self_closing {
            depth += 1;
        }
    }

    out.trim_end().to_owned()
}

/// End offset of a `<script>`/`<style>` block starting at `rest`, if any.
fn raw_block_end(rest: &str) -> Option<usize> {
    let lower = rest.to_ascii_lowercase();
    for name in ["script", "style"] {
        if lower.starts_with(&format!("<{name}")) {
            let end_tag = format!("</{name}>");
            return match lower.find(&end_tag) {
                Some(i) => Some(i + end_tag.len()),
                None => Some(rest.len()),
            };
        }
    }
    None
}

fn tag_name(inner: &str) -> String {
    inner
        .trim_start_matches('/')
        .split([' ', '\t', '\n', '/', '>'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn push_line(out: &mut String, line: &str, depth: usize) {
    if line.is_empty() {
        return;
    }
    out.push_str(&"  ".repeat(depth));
    out.push_str(line);
    out.push('\n');
}

fn push_text(out: &mut String, text: &str, depth: usize) {
    let trimmed = text.trim();
    if !trimmed.is_empty() {
        push_line(out, trimmed, depth);
    }
}

/// Visible text of an html document, tags and script/style contents removed.
pub fn strip_tags(source: &str) -> String {
    let mut out = String::with_capacity(source.len() / 2);
    let mut rest = source;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        rest = &rest[open..];
        match raw_block_end(rest) {
            Some(end) => rest = &rest[end..],
            None => match rest.find('>') {
                Some(close) => rest = &rest[close + 1..],
                None => {
                    rest = "";
                    break;
                }
            },
        }
    }
    out.push_str(rest);

    // collapse the whitespace the markup left behind
    let mut text = String::with_capacity(out.len());
    for line in out.lines() {
        let line = line.trim();
        if !line.is_empty() {
            text.push_str(line);
            text.push('\n');
        }
    }
    text.trim_end().to_owned()
}

pub fn human_size(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else if b < KB * KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else {
        format!("{:.2} GB", b / (KB * KB * KB))
    }
}

/// Classic `offset  hex  ascii` dump, capped at `max_bytes`.
pub fn hex_dump(bytes: &[u8], max_bytes: usize) -> String {
    let shown = &bytes[..bytes.len().min(max_bytes)];
    let mut out = String::with_capacity(shown.len() * 4);
    for (i, chunk) in shown.chunks(16).enumerate() {
        let hex = chunk
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        let ascii: String = chunk
            .iter()
            .map(|b| {
                if b.is_ascii_graphic() || *b == b' ' {
                    *b as char
                } else {
                    '.'
                }
            })
            .collect();
        out.push_str(&format!("{:08x}  {hex:<47}  {ascii}\n", i * 16));
    }
    if bytes.len() > shown.len() {
        out.push_str(&format!(
            "... {} more bytes ({} total)\n",
            bytes.len() - shown.len(),
            human_size(bytes.len())
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indents_xml() {
        let out = xml(r#"<?xml version="1.0"?><rss><channel><item>hi</item></channel></rss>"#);
        assert_eq!(
            out,
            "<?xml version=\"1.0\"?>\n<rss>\n  <channel>\n    <item>\n      hi\n    </item>\n  </channel>\n</rss>"
        );
    }

    #[test]
    fn void_elements_do_not_indent_forever() {
        let out = xml("<div><br><img src=x><p>a</p></div>");
        assert_eq!(out, "<div>\n  <br>\n  <img src=x>\n  <p>\n    a\n  </p>\n</div>");
    }

    #[test]
    fn script_contents_stay_verbatim() {
        let out = xml("<html><script>if (a < b) { x(); }</script></html>");
        assert!(out.contains("if (a < b) { x(); }"), "{out}");
    }

    #[test]
    fn strips_tags_and_script() {
        let html = "<html><head><style>p{color:red}</style></head><body><h1>Title</h1><p>Some <b>text</b>.</p></body></html>";
        assert_eq!(strip_tags(html), "TitleSome text.");
    }

    #[test]
    fn sizes_read_like_sizes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn hex_dump_shows_offsets_and_ascii() {
        let dump = hex_dump(b"hello", 16);
        assert_eq!(dump, "00000000  68 65 6c 6c 6f                                   hello\n");
        let capped = hex_dump(&[0u8; 40], 16);
        assert!(capped.contains("24 more bytes"), "{capped}");
    }
}
