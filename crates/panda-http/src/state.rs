//! 断点续传记录：`文件名.part.json`，和未完成的 `文件名.part` 放在一起。
//!
//! 两种记录：
//! - 分段模式（服务器支持 Range 且大小已知）：记录每一段还没下完的范围
//! - 顺序模式（大小未知，或服务器不支持 Range）：只记录已经顺序写到哪里

use std::fs::File;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::segments::Segment;

const VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResumeState {
    pub version: u32,
    /// 原始下载地址（不是跳转后的临时地址），`pandadl resume` 靠它继续。
    pub url: String,
    /// 文件总大小，未知时为空。
    pub total: Option<u64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// 是否顺序模式。
    #[serde(default)]
    pub stream: bool,
    /// 分段模式：尚未完成的段。顺序模式：只有一项，`pos` 就是已写入的字节数。
    pub segments: Vec<Segment>,
}

impl ResumeState {
    pub fn segmented(url: &str, total: u64, etag: Option<String>, last_modified: Option<String>, segments: Vec<Segment>) -> Self {
        Self { version: VERSION, url: url.into(), total: Some(total), etag, last_modified, stream: false, segments }
    }

    pub fn stream(url: &str, total: Option<u64>, etag: Option<String>, last_modified: Option<String>, written: u64) -> Self {
        Self {
            version: VERSION,
            url: url.into(),
            total,
            etag,
            last_modified,
            stream: true,
            segments: vec![Segment { start: 0, pos: written, end: total.unwrap_or(u64::MAX), active: false }],
        }
    }

    /// 顺序模式下已写入的字节数。
    pub fn stream_pos(&self) -> u64 {
        self.segments.first().map_or(0, |s| s.pos)
    }

    /// 服务器上的文件和上次一样吗？大小必须相同；ETag / Last-Modified 有就比较。
    pub fn matches(&self, total: Option<u64>, etag: Option<&str>, last_modified: Option<&str>) -> bool {
        if self.version != VERSION || self.total != total {
            return false;
        }
        // 弱 ETag（W/"…"）在不同 CDN 节点之间可能不同，只比较强 ETag。
        let strong = |e: Option<&str>| e.filter(|e| !e.starts_with("W/")).map(str::to_string);
        if let (Some(a), Some(b)) = (strong(self.etag.as_deref()), strong(etag)) {
            if a != b {
                return false;
            }
        }
        if let (Some(a), Some(b)) = (&self.last_modified, last_modified) {
            if a != b {
                return false;
            }
        }
        true
    }

    pub fn load(path: &Path) -> Option<Self> {
        let data = std::fs::read(path).ok()?;
        serde_json::from_slice(&data).ok()
    }

    /// 先写临时文件再改名，断电也不会留下半截的 JSON。
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(&tmp, path)
    }

    /// 断电安全的保存：先把数据文件刷到硬盘，再写进度记录。
    ///
    /// 记录必须在刷盘**之前**生成（调用方先 snapshot），这样记录里声称"已下载"的数据一定已经落盘。
    /// 反过来的话，断电后记录可能比数据新，续传时那一段会是全 0 的坏数据。
    pub fn save_durable(&self, data_file: &File, path: &Path) -> std::io::Result<()> {
        data_file.sync_data()?;
        self.save(path)
    }
}

pub fn part_path(final_path: &Path) -> PathBuf {
    let mut s = final_path.as_os_str().to_owned();
    s.push(".part");
    PathBuf::from(s)
}

pub fn state_path(final_path: &Path) -> PathBuf {
    let mut s = final_path.as_os_str().to_owned();
    s.push(".part.json");
    PathBuf::from(s)
}

/// 由进度记录路径反推最终文件名：`电影.mkv.part.json` → `电影.mkv`。
pub fn final_name_from_state(state_file: &Path) -> Option<String> {
    let name = state_file.file_name()?.to_str()?;
    name.strip_suffix(".part.json").filter(|n| !n.is_empty()).map(String::from)
}

/// 列出目录里所有未完成的下载：(最终文件名, 记录)。
pub fn list_unfinished(dir: &Path) -> Vec<(String, ResumeState)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<(String, ResumeState)> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            let name = final_name_from_state(&p)?;
            if !dir.join(format!("{name}.part")).exists() {
                return None;
            }
            Some((name, ResumeState::load(&p)?))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_rules() {
        let s = ResumeState::segmented("u", 100, Some("\"abc\"".into()), Some("Mon".into()), vec![]);
        assert!(s.matches(Some(100), Some("\"abc\""), Some("Mon")));
        assert!(!s.matches(Some(101), Some("\"abc\""), Some("Mon")));
        assert!(!s.matches(None, Some("\"abc\""), Some("Mon")));
        assert!(!s.matches(Some(100), Some("\"xyz\""), Some("Mon")));
        assert!(!s.matches(Some(100), None, Some("Tue")));
        assert!(s.matches(Some(100), None, None));
        assert!(s.matches(Some(100), Some("W/\"other\""), None));
        let st = ResumeState::stream("u", None, None, None, 42);
        assert!(st.matches(None, None, None));
        assert_eq!(st.stream_pos(), 42);
    }

    #[test]
    fn paths() {
        let p = Path::new(r"D:\下载\电影.mkv");
        assert_eq!(part_path(p), PathBuf::from(r"D:\下载\电影.mkv.part"));
        assert_eq!(state_path(p), PathBuf::from(r"D:\下载\电影.mkv.part.json"));
        assert_eq!(final_name_from_state(&state_path(p)).as_deref(), Some("电影.mkv"));
        assert_eq!(final_name_from_state(Path::new("x.json")), None);
    }
}
