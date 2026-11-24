//! 设置、下载历史、未完成任务的持久化（JSON 文件，存在应用数据目录）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::category::Category;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 下载根目录，各类文件默认放在它下面的子文件夹里。
    pub base_dir: PathBuf,
    /// 记住每类文件上次保存的位置。
    pub category_dirs: HashMap<String, PathBuf>,
    /// 下载前弹确认窗。
    pub ask_before_download: bool,
    /// 复制下载链接时自动弹出。
    pub watch_clipboard: bool,
    pub connections: usize,
    /// 全局限速（字节/秒），0 不限。
    pub speed_limit: u64,
    /// 启动时自动继续未完成的任务。
    pub auto_resume: bool,
    /// 悬浮窗位置（物理像素），拖动后记住。
    pub float_pos: Option<(i32, i32)>,
}

impl Default for Settings {
    fn default() -> Self {
        let base = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("熊猫下载");
        Self {
            base_dir: base,
            category_dirs: HashMap::new(),
            ask_before_download: true,
            watch_clipboard: true,
            connections: 8,
            speed_limit: 0,
            auto_resume: true,
            float_pos: None,
        }
    }
}

impl Settings {
    pub fn dir_for(&self, c: Category) -> PathBuf {
        self.category_dirs.get(c.key()).cloned().unwrap_or_else(|| self.base_dir.join(c.folder()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryItem {
    pub url: String,
    pub path: PathBuf,
    pub size: u64,
    /// Unix 秒。
    pub finished_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct History {
    pub items: Vec<HistoryItem>,
}

const HISTORY_MAX: usize = 3000;

impl History {
    pub fn add(&mut self, item: HistoryItem) {
        self.items.retain(|h| h.path != item.path);
        self.items.push(item);
        if self.items.len() > HISTORY_MAX {
            let drop = self.items.len() - HISTORY_MAX;
            self.items.drain(..drop);
        }
    }

    /// 同一个地址下载过、而且文件还在，返回那次的记录。
    pub fn find_existing(&self, url: &str) -> Option<&HistoryItem> {
        self.items.iter().rev().find(|h| h.url == url && h.path.exists())
    }
}

/// 重启后要恢复的未完成任务。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedTask {
    pub link: String,
    pub url: String,
    pub dir: PathBuf,
    /// 实际文件名（对应磁盘上的 `.part`）。
    pub name: Option<String>,
    pub paused: bool,
    #[serde(default)]
    pub video: Option<SavedVideo>,
}

/// 视频任务重启后续传需要的信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedVideo {
    pub page_url: String,
    pub choice: Option<String>,
    pub tmp_dir: PathBuf,
}

pub fn load<T: DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save<T: Serialize>(path: &Path, v: &T) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    if let Ok(data) = serde_json::to_vec_pretty(v) {
        if std::fs::write(&tmp, data).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
