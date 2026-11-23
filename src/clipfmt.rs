//! Byte-level helpers for standard clipboard formats (no Win32 calls, fully testable).

/// Builds the CF_HDROP payload (DROPFILES followed by a double-NUL-terminated wide list).
pub fn build_hdrop(paths: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&20u32.to_le_bytes()); // pFiles
    out.extend_from_slice(&0i32.to_le_bytes()); // pt.x
    out.extend_from_slice(&0i32.to_le_bytes()); // pt.y
    out.extend_from_slice(&0i32.to_le_bytes()); // fNC
    out.extend_from_slice(&1i32.to_le_bytes()); // fWide
    for path in paths {
        for unit in path.encode_utf16() {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out.extend_from_slice(&[0, 0]);
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// Parses a CF_HDROP payload. Handles both wide and ANSI (ASCII subset) lists.
pub fn parse_hdrop(data: &[u8]) -> Vec<String> {
    let Some(offset) = data.get(0..4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize) else {
        return Vec::new();
    };
    let wide = data.get(16..20).map(|b| i32::from_le_bytes(b.try_into().unwrap()) != 0).unwrap_or(true);
    let Some(list) = data.get(offset..) else { return Vec::new() };
    let mut out = Vec::new();
    if wide {
        let units: Vec<u16> = list.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        for part in units.split(|&u| u == 0) {
            if part.is_empty() {
                break;
            }
            out.push(String::from_utf16_lossy(part));
        }
    } else {
        for part in list.split(|&b| b == 0) {
            if part.is_empty() {
                break;
            }
            out.push(part.iter().map(|&b| b as char).collect());
        }
    }
    out
}

/// Reads the numeric header fields of the "HTML Format" clipboard format.
fn html_offset(data: &str, key: &str) -> Option<usize> {
    let start = data.find(key)? + key.len();
    let digits: String = data[start..].chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
    digits.parse::<i64>().ok().filter(|v| *v >= 0).map(|v| v as usize)
}

/// The fragment of an "HTML Format" payload, or the whole body if the offsets are broken.
pub fn html_fragment(data: &[u8]) -> String {
    let text = String::from_utf8_lossy(data);
    let (start, end) = (html_offset(&text, "StartFragment:"), html_offset(&text, "EndFragment:"));
    if let (Some(s), Some(e)) = (start, end) {
        if s <= e && e <= data.len() {
            return String::from_utf8_lossy(&data[s..e]).into_owned();
        }
    }
    match (text.find("<!--StartFragment-->"), text.find("<!--EndFragment-->")) {
        (Some(s), Some(e)) if s < e => text[s + 20..e].to_string(),
        _ => text.into_owned(),
    }
}

/// The "SourceURL" header of an "HTML Format" payload, if present.
pub fn html_source_url(data: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(&data[..data.len().min(1024)]);
    let line = text.lines().find(|l| l.starts_with("SourceURL:"))?;
    Some(line["SourceURL:".len()..].trim().to_string()).filter(|s| !s.is_empty())
}

/// Very small HTML to text conversion for previews (tags dropped, common entities decoded).
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut tag = String::new();
    let mut skip_depth = 0usize;
    for c in html.chars() {
        if in_tag {
            if c == '>' {
                in_tag = false;
                let name: String =
                    tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
                let name = name.to_ascii_lowercase();
                let closing = tag.starts_with('/');
                if name == "script" || name == "style" {
                    if closing {
                        skip_depth = skip_depth.saturating_sub(1);
                    } else {
                        skip_depth += 1;
                    }
                }
                if matches!(name.as_str(), "br" | "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3")
                    && !out.ends_with('\n')
                {
                    out.push('\n');
                }
                tag.clear();
            } else {
                tag.push(c);
            }
        } else if c == '<' {
            in_tag = true;
        } else if skip_depth == 0 {
            out.push(c);
        }
    }
    decode_entities(&out).trim().to_string()
}

fn decode_entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Text of a plain RTF document, good enough for a preview when no Unicode text was offered.
pub fn rtf_to_text(rtf: &[u8]) -> String {
    let text = String::from_utf8_lossy(rtf);
    let mut out = String::new();
    let mut depth = 0i32;
    let mut skip_below: Option<i32> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => depth += 1,
            '}' => {
                if skip_below == Some(depth) {
                    skip_below = None;
                }
                depth -= 1;
            }
            '\\' => {
                let mut word = String::new();
                while let Some(&n) = chars.peek() {
                    if n.is_ascii_alphabetic() {
                        word.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if word.is_empty() {
                    if let Some(n) = chars.next() {
                        if n == '*' && skip_below.is_none() {
                            skip_below = Some(depth);
                        } else if matches!(n, '\\' | '{' | '}') && skip_below.is_none() {
                            out.push(n);
                        } else if n == '\'' {
                            let hex: String = chars.by_ref().take(2).collect();
                            if skip_below.is_none() {
                                if let Ok(v) = u8::from_str_radix(&hex, 16) {
                                    out.push(v as char);
                                }
                            }
                        }
                    }
                    continue;
                }
                let mut number = String::new();
                while let Some(&n) = chars.peek() {
                    if n.is_ascii_digit() || (n == '-' && number.is_empty()) {
                        number.push(n);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if chars.peek() == Some(&' ') {
                    chars.next();
                }
                if matches!(word.as_str(), "fonttbl" | "colortbl" | "stylesheet" | "info" | "pict")
                    && skip_below.is_none()
                {
                    skip_below = Some(depth);
                }
                if skip_below.is_none() {
                    match word.as_str() {
                        "par" | "line" => out.push('\n'),
                        "tab" => out.push('\t'),
                        "u" => {
                            if let Some(ch) = number.parse::<i32>().ok().and_then(|v| char::from_u32(v as u16 as u32)) {
                                out.push(ch);
                                // Skip the ANSI fallback character.
                                if chars.peek().is_some_and(|c| *c != '\\' && *c != '{' && *c != '}') {
                                    chars.next();
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            '\r' | '\n' => {}
            _ => {
                if skip_below.is_none() && depth > 0 {
                    out.push(c);
                }
            }
        }
    }
    out.trim().to_string()
}

/// True when the text looks like a single web or mail link.
pub fn looks_like_link(text: &str) -> bool {
    let t = text.trim();
    !t.contains(char::is_whitespace)
        && (t.starts_with("http://") || t.starts_with("https://") || t.starts_with("mailto:") || t.starts_with("www."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hdrop_round_trip() {
        let files = vec![r"C:\Temp\a b.txt".to_string(), r"D:\Ünïcode\ファイル.png".to_string()];
        assert_eq!(parse_hdrop(&build_hdrop(&files)), files);
        assert!(parse_hdrop(&[]).is_empty());
        assert!(parse_hdrop(&[200, 0, 0, 0]).is_empty());
    }

    #[test]
    fn hdrop_ansi() {
        let mut data = vec![20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        data.extend_from_slice(b"C:\\a.txt\0C:\\b.txt\0\0");
        assert_eq!(parse_hdrop(&data), vec!["C:\\a.txt", "C:\\b.txt"]);
    }

    #[test]
    fn html_format_fragment() {
        let body = "<html><body><!--StartFragment--><b>Hi</b> &amp; bye<!--EndFragment--></body></html>";
        let header_len = 105 + "SourceURL:https://example.com/\r\n".len();
        let start = header_len + body.find("<!--StartFragment-->").unwrap() + 20;
        let end = header_len + body.find("<!--EndFragment-->").unwrap();
        let header = format!(
            "Version:0.9\r\nStartHTML:{:010}\r\nEndHTML:{:010}\r\nStartFragment:{:010}\r\nEndFragment:{:010}\r\nSourceURL:https://example.com/\r\n",
            header_len,
            header_len + body.len(),
            start,
            end
        );
        assert_eq!(header.len(), header_len);
        let data = format!("{header}{body}");
        assert_eq!(html_fragment(data.as_bytes()), "<b>Hi</b> &amp; bye");
        assert_eq!(html_to_text(&html_fragment(data.as_bytes())), "Hi & bye");
        assert_eq!(html_source_url(data.as_bytes()).as_deref(), Some("https://example.com/"));
    }

    #[test]
    fn html_with_broken_offsets_still_previews() {
        let data = b"Version:0.9\r\nStartFragment:-1\r\n<p>One</p><script>x()</script><p>Two</p>";
        assert_eq!(html_to_text(&html_fragment(data)), "Version:0.9\r\nStartFragment:-1\r\nOne\nTwo");
    }

    #[test]
    fn rtf_preview() {
        let rtf = br"{\rtf1\ansi{\fonttbl{\f0 Calibri;}}\f0 Hello \b world\b0\par Gr\'fc\u223?e}";
        assert_eq!(rtf_to_text(rtf), "Hello world\nGr\u{fc}\u{df}e");
    }

    #[test]
    fn links() {
        assert!(looks_like_link("https://github.com/TabanAziz"));
        assert!(looks_like_link("  www.example.com "));
        assert!(!looks_like_link("see https://example.com"));
        assert!(!looks_like_link("hello"));
    }
}
