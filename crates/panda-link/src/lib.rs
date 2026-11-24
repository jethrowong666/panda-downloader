//! 熊猫下载器 · 链接识别层
//!
//! 输入任何一条用户粘贴的链接，输出"真实地址 + 类型 + 该交给哪个下载引擎"。
//!
//! ```
//! let p = panda_link::parse("ed2k://|file|a.avi|100|0123456789abcdef0123456789abcdef|/").unwrap();
//! assert_eq!(p.engine(), panda_link::Engine::Ed2k);
//! ```

pub mod batch;
pub mod ed2k;
pub mod magnet;
pub mod wrapper;

use serde::Serialize;

pub use ed2k::{Ed2kFile, Ed2kLink};
pub use magnet::Magnet;
pub use wrapper::Wrapper;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LinkError {
    #[error("链接为空")]
    Empty,
    #[error("专用链接解包失败: {0}")]
    BadWrapper(String),
    #[error("专用链接嵌套层数过多")]
    TooManyLayers,
    #[error("磁力链接无效: {0}")]
    BadMagnet(String),
    #[error("ed2k 链接无效: {0}")]
    BadEd2k(String),
    #[error("批量模式无效: {0}")]
    BadBatch(String),
    #[error("批量展开数量过多: {0} 条（上限 {max}）", max = batch::MAX_EXPANSION)]
    BatchTooLarge(usize),
}

/// 链接类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LinkKind {
    Http { url: String },
    Ftp { url: String },
    Sftp { url: String },
    Magnet(Magnet),
    Ed2k(Ed2kLink),
    /// `.torrent` 文件，可能是网址也可能是本地路径。
    Torrent { location: String },
    /// `.metalink` / `.meta4` 多源描述文件。
    Metalink { location: String },
    /// HLS 播放列表（`.m3u8`）。
    Hls { url: String },
    /// DASH 清单（`.mpd`）。
    Dash { url: String },
    /// RTMP / RTSP / MMS / SRT 等流协议。
    Stream { protocol: String, url: String },
    Ipfs { namespace: String, path: String },
    Unknown { input: String },
}

/// 负责处理这类链接的下载引擎。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    Http,
    Ftp,
    BitTorrent,
    Ed2k,
    Metalink,
    Media,
    Ipfs,
    None,
}

impl Engine {
    /// 当前版本是否已经实现。
    pub fn implemented(self) -> bool {
        matches!(self, Engine::Http)
    }

    pub fn label(self) -> &'static str {
        match self {
            Engine::Http => "HTTP/HTTPS",
            Engine::Ftp => "FTP/SFTP",
            Engine::BitTorrent => "BT",
            Engine::Ed2k => "电驴 ed2k",
            Engine::Metalink => "Metalink 多源",
            Engine::Media => "流媒体",
            Engine::Ipfs => "IPFS",
            Engine::None => "未知",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParsedLink {
    /// 用户输入的原文。
    pub input: String,
    /// 解包后的真实地址。
    pub resolved: String,
    /// 经过了哪些专用链接包装。
    pub wrappers: Vec<Wrapper>,
    #[serde(flatten)]
    pub kind: LinkKind,
}

impl ParsedLink {
    pub fn engine(&self) -> Engine {
        match &self.kind {
            LinkKind::Http { .. } => Engine::Http,
            LinkKind::Ftp { .. } | LinkKind::Sftp { .. } => Engine::Ftp,
            LinkKind::Magnet(m) if m.btih.is_none() && m.btmh.is_none() && m.ed2k.is_some() => {
                Engine::Ed2k
            }
            LinkKind::Magnet(_) | LinkKind::Torrent { .. } => Engine::BitTorrent,
            LinkKind::Ed2k(_) => Engine::Ed2k,
            LinkKind::Metalink { .. } => Engine::Metalink,
            LinkKind::Hls { .. } | LinkKind::Dash { .. } | LinkKind::Stream { .. } => Engine::Media,
            LinkKind::Ipfs { .. } => Engine::Ipfs,
            LinkKind::Unknown { .. } => Engine::None,
        }
    }
}

/// 识别一条链接。
pub fn parse(input: &str) -> Result<ParsedLink, LinkError> {
    let trimmed = input.trim().trim_matches(|c| c == '"' || c == '\'' || c == '<' || c == '>');
    if trimmed.is_empty() {
        return Err(LinkError::Empty);
    }
    let (resolved, wrappers) = wrapper::unwrap(trimmed)?;
    let kind = classify(&resolved)?;
    Ok(ParsedLink { input: input.to_string(), resolved, wrappers, kind })
}

fn scheme_of(s: &str) -> Option<String> {
    let idx = s.find(':')?;
    let scheme = &s[..idx];
    let valid = scheme.len() >= 2
        && scheme.chars().next()?.is_ascii_alphabetic()
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.');
    // 单个字母是 Windows 盘符（C:\…），不算协议。
    valid.then(|| scheme.to_ascii_lowercase())
}

/// 去掉 ?query 和 #fragment 后的小写路径，用来判断扩展名。
fn path_lower(url: &str) -> String {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    url[..end].to_ascii_lowercase()
}

fn classify(s: &str) -> Result<LinkKind, LinkError> {
    let Some(scheme) = scheme_of(s) else {
        return Ok(classify_bare(s));
    };
    let url = s.to_string();
    Ok(match scheme.as_str() {
        "http" | "https" => {
            let p = path_lower(s);
            if p.ends_with(".torrent") {
                LinkKind::Torrent { location: url }
            } else if p.ends_with(".metalink") || p.ends_with(".meta4") {
                LinkKind::Metalink { location: url }
            } else if p.ends_with(".m3u8") {
                LinkKind::Hls { url }
            } else if p.ends_with(".mpd") {
                LinkKind::Dash { url }
            } else {
                LinkKind::Http { url }
            }
        }
        "ftp" | "ftps" => LinkKind::Ftp { url },
        "sftp" => LinkKind::Sftp { url },
        "magnet" => LinkKind::Magnet(magnet::parse(s)?),
        "ed2k" => LinkKind::Ed2k(ed2k::parse(s)?),
        "rtmp" | "rtmps" | "rtmpe" | "rtmpt" | "rtsp" | "rtsps" | "mms" | "mmsh" | "srt" => {
            LinkKind::Stream { protocol: scheme, url }
        }
        "ipfs" | "ipns" => LinkKind::Ipfs {
            namespace: scheme.clone(),
            path: s[scheme.len() + 1..].trim_start_matches('/').to_string(),
        },
        "file" => classify_bare(s),
        _ => LinkKind::Unknown { input: url },
    })
}

/// 没有协议头的输入：本地文件路径，或者裸的 info hash。
fn classify_bare(s: &str) -> LinkKind {
    let p = path_lower(s);
    if p.ends_with(".torrent") {
        return LinkKind::Torrent { location: s.to_string() };
    }
    if p.ends_with(".metalink") || p.ends_with(".meta4") {
        return LinkKind::Metalink { location: s.to_string() };
    }
    if let Ok(h) = magnet::normalize_btih(s) {
        return LinkKind::Magnet(Magnet { btih: Some(h), ..Default::default() });
    }
    LinkKind::Unknown { input: s.to_string() }
}

/// 能被识别为链接开头的协议，用于从一段文字（剪贴板、网页文本）里提取链接。
const SCHEMES: &[&str] = &[
    "https://", "http://", "ftp://", "ftps://", "sftp://", "magnet:?", "ed2k://", "thunder://",
    "flashget://", "qqdl://", "rtmp://", "rtmps://", "rtsp://", "mms://", "ipfs://", "ipns://",
];

/// 从任意文字中提取所有链接（去重，保持出现顺序）。
///
/// ed2k 链接的文件名里允许有空格，所以它一直取到同一行最后一个 `|/` 为止。
pub fn extract_links(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut starts: Vec<usize> = Vec::new();
    for sch in SCHEMES {
        let mut from = 0;
        while let Some(i) = lower[from..].find(sch) {
            let at = from + i;
            // 前一个字符是字母时（比如 "xhttp://"）不算。
            let prev_ok = at == 0 || !text[..at].chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric());
            if prev_ok && !starts.contains(&at) {
                starts.push(at);
            }
            from = at + sch.len();
        }
    }
    starts.sort_unstable();
    // "https://" 和 "http://" 不会在同一位置都命中，但 "ftps://" 里不含 "ftp://"，无需额外处理。

    let mut out: Vec<String> = Vec::new();
    for (n, &start) in starts.iter().enumerate() {
        let next = starts.get(n + 1).copied().unwrap_or(text.len());
        let seg = &text[start..next];
        let line_end = seg.find(['\n', '\r']).unwrap_or(seg.len());
        let seg = &seg[..line_end];
        let link = if lower[start..].starts_with("ed2k://") {
            match seg.rfind("|/") {
                Some(i) => &seg[..i + 2],
                None => continue,
            }
        } else {
            let end = seg
                .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`' | '，' | '。' | '）' | '】' | '」'))
                .unwrap_or(seg.len());
            seg[..end].trim_end_matches(['.', ',', ';', ':', ')', ']', '!', '?'])
        };
        if link.len() > 8 && !out.iter().any(|x| x == link) {
            out.push(link.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;

    #[test]
    fn classify_variety() {
        let cases = [
            ("https://a.com/f.zip", Engine::Http),
            ("https://a.com/x.torrent?dl=1", Engine::BitTorrent),
            ("https://a.com/live/index.M3U8", Engine::Media),
            ("https://a.com/v.mpd", Engine::Media),
            ("ftp://a.com/f", Engine::Ftp),
            ("sftp://u@a.com/f", Engine::Ftp),
            ("rtmp://a.com/live/1", Engine::Media),
            ("ipfs://bafybeigdyrzt/readme.md", Engine::Ipfs),
            ("https://a.com/f.meta4", Engine::Metalink),
            (r"C:\种子\movie.torrent", Engine::BitTorrent),
            ("c12fe1c06bba254a9dc9f519b335aa7c1367a88a", Engine::BitTorrent),
            ("magnet:?xt=urn:ed2k:31d6cfe0d16ae931b73c59d7e0c089c0", Engine::Ed2k),
            ("ed2k://|file|a.avi|100|0123456789abcdef0123456789abcdef|/", Engine::Ed2k),
            ("gopher://x", Engine::None),
        ];
        for (input, engine) in cases {
            assert_eq!(parse(input).unwrap().engine(), engine, "{input}");
        }
    }

    #[test]
    fn thunder_to_magnet() {
        let inner = "magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a";
        let link = format!(
            "thunder://{}",
            base64::engine::general_purpose::STANDARD.encode(format!("AA{inner}ZZ"))
        );
        let p = parse(&link).unwrap();
        assert_eq!(p.resolved, inner);
        assert_eq!(p.wrappers, vec![Wrapper::Thunder]);
        assert_eq!(p.engine(), Engine::BitTorrent);
    }

    #[test]
    fn extract_from_text() {
        let text = "下载地址：https://a.com/f.zip。备用 (http://b.com/g.rar)\n\
                    电驴：ed2k://|file|我的 文件.mkv|100|0123456789abcdef0123456789abcdef|/ 谢谢\n\
                    磁力 magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&dn=x 结束\n\
                    重复 https://a.com/f.zip";
        let links = extract_links(text);
        assert_eq!(
            links,
            vec![
                "https://a.com/f.zip",
                "http://b.com/g.rar",
                "ed2k://|file|我的 文件.mkv|100|0123456789abcdef0123456789abcdef|/",
                "magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&dn=x",
            ]
        );
    }

    #[test]
    fn empty_input() {
        assert_eq!(parse("   ").unwrap_err(), LinkError::Empty);
    }
}
