//! 解包迅雷 / 快车 / 旋风等"专用链接"。
//!
//! 这些链接本质上都是把真实地址做了 Base64 包装：
//! - `thunder://` = base64("AA" + url + "ZZ")
//! - `flashget://` = base64("[FLASHGET]" + url + "[FLASHGET]")，后面可能跟 `&来源标记`
//! - `qqdl://` = base64(url)
//!
//! 老链接里的真实地址经常是 GBK 编码的中文，所以解码后先按 UTF-8 尝试，不行再按 GBK。

use base64::engine::general_purpose::{GeneralPurposeConfig, GeneralPurpose};
use base64::engine::DecodePaddingMode;
use base64::{alphabet, Engine};
use serde::Serialize;

use crate::LinkError;

/// 包装层类型，按解包顺序记录。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Wrapper {
    Thunder,
    Flashget,
    Qqdl,
}

/// 最多解几层，防止恶意构造的无限嵌套。
const MAX_LAYERS: usize = 8;

const LENIENT: GeneralPurposeConfig = GeneralPurposeConfig::new()
    .with_decode_padding_mode(DecodePaddingMode::Indifferent)
    .with_decode_allow_trailing_bits(true);
const STD_LENIENT: GeneralPurpose = GeneralPurpose::new(&alphabet::STANDARD, LENIENT);
const URL_SAFE_LENIENT: GeneralPurpose = GeneralPurpose::new(&alphabet::URL_SAFE, LENIENT);

/// 反复解包，直到不再是专用链接。返回真实地址和经过的包装层。
pub fn unwrap(input: &str) -> Result<(String, Vec<Wrapper>), LinkError> {
    let mut current = input.trim().to_string();
    let mut layers = Vec::new();
    for _ in 0..MAX_LAYERS {
        match unwrap_once(&current)? {
            Some((inner, w)) => {
                layers.push(w);
                current = inner;
            }
            None => return Ok((current, layers)),
        }
    }
    Err(LinkError::TooManyLayers)
}

fn unwrap_once(s: &str) -> Result<Option<(String, Wrapper)>, LinkError> {
    if let Some(body) = strip_scheme(s, "thunder") {
        let inner = decode_with(body, |t| {
            t.strip_prefix("AA").and_then(|t| t.strip_suffix("ZZ")).map(str::to_string)
        })
        .ok_or_else(|| LinkError::BadWrapper("thunder 链接无法解码，或缺少 AA…ZZ 标记".into()))?;
        return Ok(Some((inner.trim().to_string(), Wrapper::Thunder)));
    }
    if let Some(body) = strip_scheme(s, "flashget") {
        // 快车链接末尾可能带 "&abc" 之类的来源标记，Base64 字母表里没有 '&'，直接截掉。
        let body = body.split('&').next().unwrap_or(body);
        let inner = decode_with(body, |t| {
            t.strip_prefix("[FLASHGET]")
                .and_then(|t| t.strip_suffix("[FLASHGET]"))
                .map(str::to_string)
        })
        .ok_or_else(|| {
            LinkError::BadWrapper("flashget 链接无法解码，或缺少 [FLASHGET] 标记".into())
        })?;
        return Ok(Some((inner.trim().to_string(), Wrapper::Flashget)));
    }
    if let Some(body) = strip_scheme(s, "qqdl") {
        let inner = decode_with(body, |t| {
            let t = t.trim();
            t.contains("://").then(|| t.to_string())
        })
        .ok_or_else(|| LinkError::BadWrapper("qqdl 链接无法解码".into()))?;
        return Ok(Some((inner, Wrapper::Qqdl)));
    }
    Ok(None)
}

/// 依次尝试几种解码方式，返回第一个通过 `accept` 检查的结果。
///
/// 网页里抄来的链接末尾常多一个 '/'，但 '/' 也是合法的 Base64 字符，
/// 所以先按原样解，不行再去掉末尾的 '/' 重试。
fn decode_with(body: &str, accept: impl Fn(&str) -> Option<String>) -> Option<String> {
    // 有的网页会把 Base64 再做一次百分号编码（%2B、%2F、%3D），先还原。
    let body = percent_encoding::percent_decode_str(body).decode_utf8_lossy();
    let cleaned: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let trimmed = cleaned.trim_end_matches('/');
    for candidate in [cleaned.as_str(), trimmed] {
        for engine in [&STD_LENIENT, &URL_SAFE_LENIENT] {
            if let Ok(bytes) = engine.decode(candidate) {
                if let Some(v) = accept(&bytes_to_text(&bytes)) {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// 去掉 `scheme://`（不区分大小写），返回后面的部分。
fn strip_scheme<'a>(s: &'a str, scheme: &str) -> Option<&'a str> {
    let prefix_len = scheme.len() + 3;
    // 用 get 而不是直接切片：输入可能是中文路径，切在字符中间会 panic。
    let head = s.get(..scheme.len())?;
    let sep = s.get(scheme.len()..prefix_len)?;
    (head.eq_ignore_ascii_case(scheme) && sep == "://").then(|| &s[prefix_len..])
}

/// 先按 UTF-8 解码，失败再按 GBK（GB18030 兼容 GBK）。
pub(crate) fn bytes_to_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let (text, _, _) = encoding_rs::GB18030.decode(bytes);
            text.into_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;

    #[test]
    fn thunder() {
        let url = "http://example.com/a.zip";
        let link = format!("thunder://{}", STANDARD.encode(format!("AA{url}ZZ")));
        let (out, layers) = unwrap(&link).unwrap();
        assert_eq!(out, url);
        assert_eq!(layers, vec![Wrapper::Thunder]);
    }

    #[test]
    fn thunder_uppercase_scheme_and_trailing_slash() {
        let url = "http://example.com/a.zip";
        let link = format!("Thunder://{}/", STANDARD.encode(format!("AA{url}ZZ")));
        assert_eq!(unwrap(&link).unwrap().0, url);
    }

    #[test]
    fn thunder_gbk_chinese() {
        let url = "http://example.com/中文电影.mkv";
        let text = format!("AA{url}ZZ");
        let (gbk, _, _) = encoding_rs::GBK.encode(&text);
        let link = format!("thunder://{}", STANDARD.encode(&gbk));
        assert_eq!(unwrap(&link).unwrap().0, url);
    }

    #[test]
    fn thunder_wrapping_ed2k() {
        let ed2k = "ed2k://|file|a.avi|100|0123456789ABCDEF0123456789ABCDEF|/";
        let link = format!("thunder://{}", STANDARD.encode(format!("AA{ed2k}ZZ")));
        assert_eq!(unwrap(&link).unwrap().0, ed2k);
    }

    #[test]
    fn flashget_with_suffix() {
        let url = "ftp://example.com/b.iso";
        let link = format!(
            "flashget://{}&abc",
            STANDARD.encode(format!("[FLASHGET]{url}[FLASHGET]"))
        );
        let (out, layers) = unwrap(&link).unwrap();
        assert_eq!(out, url);
        assert_eq!(layers, vec![Wrapper::Flashget]);
    }

    #[test]
    fn qqdl() {
        let url = "https://example.com/c.exe";
        let link = format!("qqdl://{}", STANDARD.encode(url));
        assert_eq!(unwrap(&link).unwrap().0, url);
    }

    #[test]
    fn nested_layers() {
        let url = "https://example.com/d.bin";
        let inner = format!("qqdl://{}", STANDARD.encode(url));
        let outer = format!("thunder://{}", STANDARD.encode(format!("AA{inner}ZZ")));
        let (out, layers) = unwrap(&outer).unwrap();
        assert_eq!(out, url);
        assert_eq!(layers, vec![Wrapper::Thunder, Wrapper::Qqdl]);
    }

    #[test]
    fn missing_padding_and_percent_encoded() {
        let url = "http://example.com/x";
        let b64 = STANDARD.encode(format!("AA{url}ZZ"));
        let no_pad = b64.trim_end_matches('=');
        assert_eq!(unwrap(&format!("thunder://{no_pad}")).unwrap().0, url);
        let pct = b64.replace('=', "%3D").replace('+', "%2B").replace('/', "%2F");
        assert_eq!(unwrap(&format!("thunder://{pct}")).unwrap().0, url);
    }

    #[test]
    fn plain_url_untouched() {
        let (out, layers) = unwrap("  https://example.com/  ").unwrap();
        assert_eq!(out, "https://example.com/");
        assert!(layers.is_empty());
    }

    #[test]
    fn bad_thunder() {
        assert!(unwrap("thunder://!!!").is_err());
        let link = format!("thunder://{}", STANDARD.encode("no markers"));
        assert!(unwrap(&link).is_err());
    }
}
