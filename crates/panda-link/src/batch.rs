//! 批量链接展开：`http://x/img[001-120].jpg`、`http://x/[a-f].zip`、`[0-100:5]`。
//!
//! 只识别 `[数字-数字]` 和 `[单个字母-单个字母]`，所以 IPv6 地址 `http://[::1]/` 不受影响。
//! 数字起点带前导零时（如 001），结果保持同样位数。

use crate::LinkError;

/// 一次最多展开多少条，防止误操作卡死。
pub const MAX_EXPANSION: usize = 100_000;

#[derive(Debug, Clone, PartialEq)]
enum Piece {
    Text(String),
    Range(Vec<String>),
}

/// 展开批量模式。没有模式时返回原字符串本身。
pub fn expand(pattern: &str) -> Result<Vec<String>, LinkError> {
    let pieces = tokenize(pattern)?;
    let mut total: usize = 1;
    for p in &pieces {
        if let Piece::Range(v) = p {
            total = total.saturating_mul(v.len());
        }
    }
    if total > MAX_EXPANSION {
        return Err(LinkError::BatchTooLarge(total));
    }
    let mut out = vec![String::new()];
    for p in pieces {
        match p {
            Piece::Text(t) => out.iter_mut().for_each(|s| s.push_str(&t)),
            Piece::Range(vals) => {
                out = out
                    .iter()
                    .flat_map(|prefix| vals.iter().map(move |v| format!("{prefix}{v}")))
                    .collect();
            }
        }
    }
    Ok(out)
}

/// 字符串里是否含有批量模式。
pub fn has_pattern(s: &str) -> bool {
    tokenize(s).map(|p| p.iter().any(|x| matches!(x, Piece::Range(_)))).unwrap_or(false)
}

fn tokenize(s: &str) -> Result<Vec<Piece>, LinkError> {
    let mut pieces = Vec::new();
    let mut text = String::new();
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        let Some(close_rel) = rest[open..].find(']') else { break };
        let inner = &rest[open + 1..open + close_rel];
        match parse_range(inner)? {
            Some(vals) => {
                text.push_str(&rest[..open]);
                if !text.is_empty() {
                    pieces.push(Piece::Text(std::mem::take(&mut text)));
                }
                pieces.push(Piece::Range(vals));
            }
            None => text.push_str(&rest[..open + close_rel + 1]),
        }
        rest = &rest[open + close_rel + 1..];
    }
    text.push_str(rest);
    if !text.is_empty() {
        pieces.push(Piece::Text(text));
    }
    Ok(pieces)
}

fn parse_range(inner: &str) -> Result<Option<Vec<String>>, LinkError> {
    let (body, step) = match inner.split_once(':') {
        Some((b, st)) if !st.is_empty() && st.chars().all(|c| c.is_ascii_digit()) => {
            (b, st.parse::<u64>().map_err(|_| LinkError::BadBatch(inner.into()))?)
        }
        Some(_) => return Ok(None),
        None => (inner, 1),
    };
    let Some((a, b)) = body.split_once('-') else { return Ok(None) };
    if step == 0 {
        return Err(LinkError::BadBatch("步长不能为 0".into()));
    }

    if !a.is_empty() && !b.is_empty() && a.chars().all(|c| c.is_ascii_digit()) && b.chars().all(|c| c.is_ascii_digit()) {
        let width = if a.len() > 1 && a.starts_with('0') { a.len() } else { 0 };
        let (x, y): (u64, u64) = (
            a.parse().map_err(|_| LinkError::BadBatch(inner.into()))?,
            b.parse().map_err(|_| LinkError::BadBatch(inner.into()))?,
        );
        let count = x.abs_diff(y) / step + 1;
        if count as usize > MAX_EXPANSION {
            return Err(LinkError::BatchTooLarge(count as usize));
        }
        let vals = (0..count)
            .map(|i| if x <= y { x + i * step } else { x - i * step })
            .map(|n| format!("{n:0width$}"))
            .collect();
        return Ok(Some(vals));
    }

    let (ca, cb) = (single_alpha(a), single_alpha(b));
    if let (Some(ca), Some(cb)) = (ca, cb) {
        if ca.is_ascii_lowercase() != cb.is_ascii_lowercase() {
            return Ok(None);
        }
        let (x, y) = (ca as u8, cb as u8);
        let range: Vec<u8> = if x <= y {
            (x..=y).step_by(step as usize).collect()
        } else {
            (y..=x).rev().step_by(step as usize).collect()
        };
        return Ok(Some(range.into_iter().map(|c| (c as char).to_string()).collect()));
    }
    Ok(None)
}

fn single_alpha(s: &str) -> Option<char> {
    let mut it = s.chars();
    match (it.next(), it.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => Some(c),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_padded() {
        let v = expand("http://x/img[008-011].jpg").unwrap();
        assert_eq!(v, vec!["http://x/img008.jpg", "http://x/img009.jpg", "http://x/img010.jpg", "http://x/img011.jpg"]);
    }

    #[test]
    fn letters_and_product() {
        let v = expand("http://x/[a-b]/[1-2].zip").unwrap();
        assert_eq!(v, vec!["http://x/a/1.zip", "http://x/a/2.zip", "http://x/b/1.zip", "http://x/b/2.zip"]);
    }

    #[test]
    fn step_and_reverse() {
        assert_eq!(expand("[0-10:5]").unwrap(), vec!["0", "5", "10"]);
        assert_eq!(expand("[3-1]").unwrap(), vec!["3", "2", "1"]);
    }

    #[test]
    fn ipv6_and_plain_untouched() {
        assert_eq!(expand("http://[::1]:8080/a").unwrap(), vec!["http://[::1]:8080/a"]);
        assert_eq!(expand("http://x/[readme].txt").unwrap(), vec!["http://x/[readme].txt"]);
        assert!(!has_pattern("http://[2001:db8::1]/"));
        assert!(has_pattern("http://x/[1-3]"));
    }

    #[test]
    fn too_large() {
        assert!(expand("[1-1000][1-1000]").is_err());
    }
}
