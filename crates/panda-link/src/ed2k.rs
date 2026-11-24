//! 电驴（eDonkey2000）链接解析。
//!
//! 文件链接：`ed2k://|file|文件名|大小|MD4哈希|[h=AICH根哈希|][p=分块哈希|][/|sources,ip:port,…|]/`
//! 服务器链接：`ed2k://|server|IP|端口|/`
//! 服务器列表：`ed2k://|serverlist|URL|/`

use serde::Serialize;

use crate::LinkError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Ed2kLink {
    File(Ed2kFile),
    Server { host: String, port: u16 },
    ServerList { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ed2kFile {
    pub name: String,
    pub size: u64,
    /// MD4 哈希，32 位小写十六进制。
    pub hash: String,
    /// AICH 根哈希（Base32），用于损坏块的精确定位。
    pub aich: Option<String>,
    /// 每 9.28MB 分块的 MD4 哈希。
    pub part_hashes: Vec<String>,
    /// 链接里附带的 `ip:port` 来源。
    pub sources: Vec<String>,
}

/// ed2k 的分块大小：9500 KiB。
pub const ED2K_PART_SIZE: u64 = 9_728_000;

/// 电驴网络允许的最大文件大小（256 GiB）。
const MAX_SIZE: u64 = 256 * 1024 * 1024 * 1024;

pub fn parse(s: &str) -> Result<Ed2kLink, LinkError> {
    let bad = |m: &str| LinkError::BadEd2k(m.to_string());
    let rest = s
        .get(..7)
        .filter(|p| p.eq_ignore_ascii_case("ed2k://"))
        .map(|_| &s[7..])
        .ok_or_else(|| bad("不是 ed2k:// 链接"))?;
    // 网页上的链接有时把 '|' 编码成 %7C。
    let rest = rest.replace("%7C", "|").replace("%7c", "|");
    let parts: Vec<&str> = rest.split('|').collect();
    if parts.len() < 3 || !parts[0].is_empty() {
        return Err(bad("格式不对，应以 ed2k://| 开头"));
    }

    match parts[1].to_ascii_lowercase().as_str() {
        "file" => {
            if parts.len() < 5 {
                return Err(bad("文件链接缺少 名称|大小|哈希"));
            }
            let name = percent_encoding::percent_decode_str(parts[2])
                .decode_utf8_lossy()
                .trim()
                .to_string();
            if name.is_empty() {
                return Err(bad("文件名为空"));
            }
            let size: u64 = parts[3].trim().parse().map_err(|_| bad("文件大小不是数字"))?;
            if size == 0 || size > MAX_SIZE {
                return Err(bad("文件大小超出范围"));
            }
            let hash = parts[4].trim().to_ascii_lowercase();
            if hash.len() != 32 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(bad("MD4 哈希必须是 32 位十六进制"));
            }
            let mut file = Ed2kFile {
                name,
                size,
                hash,
                aich: None,
                part_hashes: Vec::new(),
                sources: Vec::new(),
            };
            for p in &parts[5..] {
                let p = p.trim();
                if let Some(v) = p.strip_prefix("h=") {
                    file.aich = Some(v.to_ascii_uppercase());
                } else if let Some(v) = p.strip_prefix("p=") {
                    file.part_hashes =
                        v.split(':').filter(|h| h.len() == 32).map(|h| h.to_ascii_lowercase()).collect();
                } else if let Some(v) = p.strip_prefix("sources,") {
                    file.sources =
                        v.split(',').map(str::trim).filter(|x| !x.is_empty()).map(String::from).collect();
                }
            }
            Ok(Ed2kLink::File(file))
        }
        "server" => {
            if parts.len() < 4 {
                return Err(bad("服务器链接缺少 IP|端口"));
            }
            let port = parts[3].trim().parse().map_err(|_| bad("端口不是数字"))?;
            Ok(Ed2kLink::Server { host: parts[2].trim().to_string(), port })
        }
        "serverlist" => Ok(Ed2kLink::ServerList { url: parts[2].trim().to_string() }),
        other => Err(bad(&format!("不支持的 ed2k 类型: {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_link() {
        let l = parse("ed2k://|file|%E7%94%B5%E5%BD%B1.mkv|1468006400|0123456789ABCDEF0123456789ABCDEF|h=ABCDEFGHIJKLMNOPQRSTUVWXYZ234567|/").unwrap();
        let Ed2kLink::File(f) = l else { panic!() };
        assert_eq!(f.name, "电影.mkv");
        assert_eq!(f.size, 1_468_006_400);
        assert_eq!(f.hash, "0123456789abcdef0123456789abcdef");
        assert!(f.aich.is_some());
    }

    #[test]
    fn file_link_raw_chinese_and_sources() {
        let l = parse("ed2k://|file|中文 名字.rar|1000|0123456789abcdef0123456789abcdef|/|sources,1.2.3.4:4662,5.6.7.8:4662|/").unwrap();
        let Ed2kLink::File(f) = l else { panic!() };
        assert_eq!(f.name, "中文 名字.rar");
        assert_eq!(f.sources, vec!["1.2.3.4:4662", "5.6.7.8:4662"]);
    }

    #[test]
    fn encoded_pipes() {
        let l = parse("ed2k://%7Cfile%7Ca.txt%7C5%7C0123456789abcdef0123456789abcdef%7C/").unwrap();
        assert!(matches!(l, Ed2kLink::File(_)));
    }

    #[test]
    fn server_links() {
        assert_eq!(
            parse("ed2k://|server|45.82.80.155|5687|/").unwrap(),
            Ed2kLink::Server { host: "45.82.80.155".into(), port: 5687 }
        );
        assert!(matches!(
            parse("ed2k://|serverlist|http://x/server.met|/").unwrap(),
            Ed2kLink::ServerList { .. }
        ));
    }

    #[test]
    fn bad_links() {
        assert!(parse("ed2k://|file|a|notnum|0123456789abcdef0123456789abcdef|/").is_err());
        assert!(parse("ed2k://|file|a|10|short|/").is_err());
        assert!(parse("ed2k://|file|a|0|0123456789abcdef0123456789abcdef|/").is_err());
        assert!(parse("ed2k://|foo|a|/").is_err());
    }
}
