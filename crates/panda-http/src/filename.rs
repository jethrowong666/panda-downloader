//! 文件名推断与清理。

use std::path::{Path, PathBuf};

/// 从 `Content-Disposition` 头里取文件名。支持 RFC 5987 的 `filename*=UTF-8''…`，
/// 也兼容国内服务器常见的"直接塞 GBK/UTF-8 原始字节"的写法。
pub fn from_content_disposition(raw: &[u8]) -> Option<String> {
    let text = bytes_to_text(raw);
    let mut plain: Option<String> = None;
    for part in split_params(&text) {
        let part = part.trim();
        let Some((k, v)) = part.split_once('=') else { continue };
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim();
        if k == "filename*" {
            // charset'lang'percent-encoded
            let mut it = v.splitn(3, '\'');
            let (charset, _lang, enc) = (it.next()?, it.next()?, it.next()?);
            let bytes: Vec<u8> = percent_encoding::percent_decode_str(enc.trim_matches('"')).collect();
            let name = if charset.eq_ignore_ascii_case("utf-8") {
                String::from_utf8_lossy(&bytes).into_owned()
            } else {
                bytes_to_text(&bytes)
            };
            if !name.is_empty() {
                return Some(name);
            }
        } else if k == "filename" {
            let v = v.trim_matches('"');
            // 有的服务器把文件名做了百分号编码但不按规范写 filename*。
            let decoded = if v.contains('%') {
                let b: Vec<u8> = percent_encoding::percent_decode_str(v).collect();
                bytes_to_text(&b)
            } else {
                v.to_string()
            };
            plain = Some(decoded);
        }
    }
    plain.filter(|s| !s.is_empty())
}

/// 按 `;` 分割参数，但忽略引号里的分号。
fn split_params(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut in_quote) = (0, false);
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            ';' if !in_quote => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// 从网址路径的最后一段取文件名。
pub fn from_url(url: &url::Url) -> Option<String> {
    let last = url.path_segments()?.rfind(|s| !s.is_empty())?;
    let bytes: Vec<u8> = percent_encoding::percent_decode_str(last).collect();
    let name = bytes_to_text(&bytes);
    (!name.is_empty()).then_some(name)
}

fn bytes_to_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::GB18030.decode(bytes).0.into_owned(),
    }
}

/// 清理成 Windows / macOS / Linux 都合法的文件名。
pub fn sanitize(name: &str) -> String {
    // 去掉路径部分，防止 "../../evil" 之类的路径穿越。
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let mut s: String = base
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    s = s.trim().trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() || s == "." || s == ".." {
        return "download".into();
    }
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem.as_str()) {
        s.insert(0, '_');
    }
    truncate_keep_ext(&s, 200)
}

/// 文件名太长时截断主体部分，保留扩展名。
fn truncate_keep_ext(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let (stem, ext) = match s.rfind('.') {
        Some(i) if s.len() - i <= 16 => (&s[..i], &s[i..]),
        _ => (s, ""),
    };
    let mut cut = max_bytes.saturating_sub(ext.len());
    while !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}{}", &stem[..cut], ext)
}

/// 目标文件已存在时，生成 "名字 (1).ext"、"名字 (2).ext"……
pub fn unique_path(dir: &Path, name: &str, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let first = dir.join(name);
    if !taken(&first) {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_disposition_variants() {
        assert_eq!(from_content_disposition(b"attachment; filename=\"a b.zip\"").as_deref(), Some("a b.zip"));
        assert_eq!(
            from_content_disposition(b"attachment; filename=\"fallback.zip\"; filename*=UTF-8''%E4%B8%AD%E6%96%87.zip").as_deref(),
            Some("中文.zip")
        );
        assert_eq!(from_content_disposition(b"inline; filename=%E4%B8%AD.txt").as_deref(), Some("中.txt"));
        let (gbk, _, _) = encoding_rs::GBK.encode("attachment; filename=\"电影.mkv\"");
        assert_eq!(from_content_disposition(&gbk).as_deref(), Some("电影.mkv"));
        assert_eq!(from_content_disposition(b"attachment; filename=\"semi;colon.txt\"").as_deref(), Some("semi;colon.txt"));
        assert_eq!(from_content_disposition(b"attachment"), None);
    }

    #[test]
    fn url_names() {
        let u = url::Url::parse("https://x.com/dir/%E6%96%87%E4%BB%B6.pdf?token=1").unwrap();
        assert_eq!(from_url(&u).as_deref(), Some("文件.pdf"));
        assert_eq!(from_url(&url::Url::parse("https://x.com/").unwrap()), None);
    }

    #[test]
    fn sanitize_names() {
        assert_eq!(sanitize("../../etc/passwd"), "passwd");
        assert_eq!(sanitize(r"..\..\win.ini"), "win.ini");
        assert_eq!(sanitize("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(sanitize("CON.txt"), "_CON.txt");
        assert_eq!(sanitize("name. . "), "name");
        assert_eq!(sanitize(""), "download");
        let long = format!("{}.mp4", "长".repeat(100));
        let s = sanitize(&long);
        assert!(s.len() <= 200 && s.ends_with(".mp4"));
    }

    #[test]
    fn unique() {
        let dir = Path::new("d");
        let taken = |p: &Path| p == dir.join("a.zip") || p == dir.join("a (1).zip");
        assert_eq!(unique_path(dir, "a.zip", taken), dir.join("a (2).zip"));
        assert_eq!(unique_path(dir, "b", taken), dir.join("b"));
    }
}
