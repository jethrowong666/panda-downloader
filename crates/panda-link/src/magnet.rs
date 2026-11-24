//! 磁力链接解析。
//!
//! 一个磁力链接可以同时带多种哈希（BT v1/v2、ed2k、SHA-1），
//! 这正是"多源下载"的入口：同一个文件可以同时从 BT 和电驴网络获取。

use serde::Serialize;
use url::Url;

use crate::LinkError;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Magnet {
    /// BT v1 info hash，统一转成 40 位小写十六进制。
    pub btih: Option<String>,
    /// BT v2 multihash（`urn:btmh:1220…`），原样保留十六进制。
    pub btmh: Option<String>,
    /// 电驴 MD4 哈希（32 位小写十六进制）。
    pub ed2k: Option<String>,
    /// SHA-1（Base32 原样保留）。
    pub sha1: Option<String>,
    /// 显示名称。
    pub name: Option<String>,
    /// 文件大小（字节）。
    pub size: Option<u64>,
    pub trackers: Vec<String>,
    /// Web Seed（BEP 19），也就是可以直接走 HTTP 下载的镜像。
    pub web_seeds: Vec<String>,
    /// `as=` 可直接下载的地址，`xs=` 种子文件地址。
    pub sources: Vec<String>,
}

impl Magnet {
    pub fn is_empty(&self) -> bool {
        self.btih.is_none() && self.btmh.is_none() && self.ed2k.is_none() && self.sha1.is_none()
    }
}

pub fn parse(s: &str) -> Result<Magnet, LinkError> {
    let url = Url::parse(s).map_err(|e| LinkError::BadMagnet(e.to_string()))?;
    let mut m = Magnet::default();
    for (key, value) in url.query_pairs() {
        let value = value.trim().to_string();
        // 带编号的参数（xt.1、tr.2）按同名处理。
        let key = key.split('.').next().unwrap_or("").to_ascii_lowercase();
        match key.as_str() {
            "xt" => apply_xt(&mut m, &value)?,
            "dn" => m.name = Some(value),
            "xl" => m.size = value.parse().ok(),
            "tr" => push_unique(&mut m.trackers, value),
            "ws" => push_unique(&mut m.web_seeds, value),
            "as" | "xs" => push_unique(&mut m.sources, value),
            _ => {}
        }
    }
    if m.is_empty() {
        return Err(LinkError::BadMagnet("缺少 xt 哈希参数".into()));
    }
    Ok(m)
}

fn push_unique(v: &mut Vec<String>, s: String) {
    if !s.is_empty() && !v.contains(&s) {
        v.push(s);
    }
}

fn apply_xt(m: &mut Magnet, xt: &str) -> Result<(), LinkError> {
    let lower = xt.to_ascii_lowercase();
    if let Some(h) = lower.strip_prefix("urn:btih:") {
        let raw = &xt[xt.len() - h.len()..];
        m.btih = Some(normalize_btih(raw)?);
    } else if let Some(h) = lower.strip_prefix("urn:btmh:") {
        if !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(LinkError::BadMagnet("btmh 不是十六进制".into()));
        }
        m.btmh = Some(h.to_string());
    } else if let Some(h) = lower.strip_prefix("urn:ed2k:") {
        if h.len() != 32 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(LinkError::BadMagnet("ed2k 哈希必须是 32 位十六进制".into()));
        }
        m.ed2k = Some(h.to_string());
    } else if let Some(h) = lower.strip_prefix("urn:sha1:") {
        m.sha1 = Some(h.to_ascii_uppercase());
    }
    // 其他类型（tree:tiger、md5 等）暂时忽略，不报错。
    Ok(())
}

/// BT v1 info hash 可能是 40 位十六进制，也可能是 32 位 Base32。
pub fn normalize_btih(h: &str) -> Result<String, LinkError> {
    if h.len() == 40 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(h.to_ascii_lowercase());
    }
    if h.len() == 32 {
        if let Some(bytes) = base32_decode(h) {
            if bytes.len() == 20 {
                return Ok(bytes.iter().map(|b| format!("{b:02x}")).collect());
            }
        }
    }
    Err(LinkError::BadMagnet(format!("无效的 info hash: {h}")))
}

/// RFC 4648 Base32（无填充），只用于 info hash。
fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let (mut buf, mut bits) = (0u64, 0u32);
    for c in s.chars() {
        let v = match c.to_ascii_uppercase() {
            c @ 'A'..='Z' => c as u64 - 'A' as u64,
            c @ '2'..='7' => c as u64 - '2' as u64 + 26,
            _ => return None,
        };
        buf = (buf << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base32_encode(data: &[u8]) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
        let (mut out, mut buf, mut bits) = (String::new(), 0u64, 0u32);
        for &b in data {
            buf = (buf << 8) | b as u64;
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                out.push(A[((buf >> bits) & 31) as usize] as char);
            }
        }
        if bits > 0 {
            out.push(A[((buf << (5 - bits)) & 31) as usize] as char);
        }
        out
    }

    #[test]
    fn hex_magnet_with_trackers() {
        let m = parse("magnet:?xt=urn:btih:C12FE1C06BBA254A9DC9F519B335AA7C1367A88A&dn=Ubuntu+24.04&xl=123&tr=udp%3A%2F%2Ft1%3A80&tr=udp%3A%2F%2Ft2%3A80&tr=udp%3A%2F%2Ft1%3A80").unwrap();
        assert_eq!(m.btih.as_deref(), Some("c12fe1c06bba254a9dc9f519b335aa7c1367a88a"));
        assert_eq!(m.name.as_deref(), Some("Ubuntu 24.04"));
        assert_eq!(m.size, Some(123));
        assert_eq!(m.trackers, vec!["udp://t1:80", "udp://t2:80"]);
    }

    #[test]
    fn base32_hash() {
        // 同一个哈希的 Base32 形式（RFC 4648 测试向量："foobar" → MZXW6YTBOI）
        assert_eq!(base32_decode("MZXW6YTBOI").unwrap(), b"foobar");
        let hex = "c12fe1c06bba254a9dc9f519b335aa7c1367a88a";
        let bytes: Vec<u8> = (0..20).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect();
        let b32 = base32_encode(&bytes);
        assert_eq!(b32.len(), 32);
        let m = parse(&format!("magnet:?xt=urn:btih:{b32}")).unwrap();
        assert_eq!(m.btih.as_deref(), Some(hex));
    }

    #[test]
    fn multi_hash_magnet() {
        let m = parse("magnet:?xt.1=urn:ed2k:31D6CFE0D16AE931B73C59D7E0C089C0&xt.2=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&ws=http%3A%2F%2Fmirror%2Ff").unwrap();
        assert_eq!(m.ed2k.as_deref(), Some("31d6cfe0d16ae931b73c59d7e0c089c0"));
        assert!(m.btih.is_some());
        assert_eq!(m.web_seeds, vec!["http://mirror/f"]);
    }

    #[test]
    fn rejects_empty() {
        assert!(parse("magnet:?dn=nothing").is_err());
        assert!(parse("magnet:?xt=urn:btih:xyz").is_err());
    }
}
