//! 熊猫下载器 · HTTP/HTTPS 下载引擎
//!
//! - 多连接分段下载，下载中途动态劈段（见 [`segments`]），所有连接几乎同时结束
//! - 断点续传：`.part` 数据文件 + `.part.json` 进度记录，用 ETag / Last-Modified 校验服务器文件是否变化
//! - 链接续期：CDN 跳转后的临时地址过期（403/410）时，回到原始地址重新获取
//! - 继承浏览器上下文：Cookie、Referer、User-Agent、自定义请求头
//! - 全局或单任务限速，运行中可调
//! - 服务器不支持分段时自动退回单连接

pub mod filename;
pub mod limiter;
pub mod segments;
pub mod state;

mod download;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use download::{Downloader, PauseHandle, Task};
pub use limiter::RateLimiter;

/// 客户端级别的设置（所有任务共用）。
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub user_agent: String,
    pub connect_timeout: Duration,
    /// 代理，例如 `http://127.0.0.1:7890`、`socks5://127.0.0.1:1080`。
    pub proxy: Option<String>,
    /// 直接连接，不使用系统代理和环境变量里的代理（国内网站用，避免绕到海外被当成境外访问）。
    pub direct: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            // 很多网站会拒绝"不像浏览器"的请求，默认伪装成常见浏览器。
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36".into(),
            connect_timeout: Duration::from_secs(15),
            proxy: None,
            direct: false,
        }
    }
}

/// 单个任务的设置。
#[derive(Debug, Clone)]
pub struct TaskOptions {
    /// 最大连接数。
    pub connections: usize,
    /// 每段最小字节数，低于两倍这个值的段不再劈开。
    pub min_split: u64,
    pub out_dir: PathBuf,
    /// 指定文件名；不指定时从服务器响应或网址推断。
    pub file_name: Option<String>,
    pub referer: Option<String>,
    pub cookie: Option<String>,
    /// 覆盖默认 User-Agent（浏览器扩展会传浏览器自己的 UA）。
    pub user_agent: Option<String>,
    pub headers: Vec<(String, String)>,
    /// 连续失败多少次（期间没有任何进展）后放弃。
    pub max_retries: u32,
    /// 多久收不到数据算超时。
    pub read_timeout: Duration,
}

impl Default for TaskOptions {
    fn default() -> Self {
        Self {
            connections: 8,
            min_split: 1024 * 1024,
            out_dir: PathBuf::from("."),
            file_name: None,
            referer: None,
            cookie: None,
            user_agent: None,
            headers: Vec::new(),
            max_retries: 8,
            read_timeout: Duration::from_secs(30),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("网址无效: {0}")]
    InvalidUrl(String),
    #[error("网络错误: {0}")]
    Network(String),
    #[error("服务器返回 HTTP {0}")]
    Status(u16),
    #[error("连接超时，{0} 秒没有收到数据")]
    Timeout(u64),
    #[error("服务器上的文件已经变化，无法续传")]
    RemoteChanged,
    #[error("服务器不再支持分段下载")]
    RangeUnsupported,
    #[error("重试 {attempts} 次后仍然失败: {last}")]
    RetriesExhausted { attempts: u32, last: String },
    #[error("磁盘读写失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("已暂停")]
    Cancelled,
}

impl DownloadError {
    /// 重试也没用的错误。
    pub(crate) fn is_fatal(&self) -> bool {
        match self {
            DownloadError::Status(code) => matches!(code, 400 | 401 | 403 | 404 | 405 | 410 | 451),
            DownloadError::InvalidUrl(_)
            | DownloadError::RemoteChanged
            | DownloadError::RangeUnsupported
            | DownloadError::Io(_)
            | DownloadError::Cancelled
            | DownloadError::RetriesExhausted { .. } => true,
            DownloadError::Network(_) | DownloadError::Timeout(_) => false,
        }
    }
}

impl From<reqwest::Error> for DownloadError {
    fn from(e: reqwest::Error) -> Self {
        if let Some(s) = e.status() {
            return DownloadError::Status(s.as_u16());
        }
        // 把底层原因串起来，方便排查（例如 TLS 证书错误）。
        let mut msg = e.to_string();
        let mut src = std::error::Error::source(&e);
        while let Some(s) = src {
            msg.push_str(": ");
            msg.push_str(&s.to_string());
            src = s.source();
        }
        DownloadError::Network(msg)
    }
}

/// 任务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Status {
    Probing = 0,
    Downloading = 1,
    Finishing = 2,
    Done = 3,
    Paused = 4,
    Failed = 5,
}

/// 实时进度，界面线程可以随时读取，不需要加锁等待下载线程。
#[derive(Debug)]
pub struct Progress {
    total: AtomicU64,
    downloaded: AtomicU64,
    connections: AtomicUsize,
    status: AtomicU8,
    resumable: AtomicBool,
    path: Mutex<Option<PathBuf>>,
}

const UNKNOWN: u64 = u64::MAX;

impl Default for Progress {
    fn default() -> Self {
        Self {
            total: AtomicU64::new(UNKNOWN),
            downloaded: AtomicU64::new(0),
            connections: AtomicUsize::new(0),
            status: AtomicU8::new(Status::Probing as u8),
            resumable: AtomicBool::new(false),
            path: Mutex::new(None),
        }
    }
}

impl Progress {
    /// 文件总大小，未知时为 `None`。
    pub fn total(&self) -> Option<u64> {
        match self.total.load(Ordering::Relaxed) {
            UNKNOWN => None,
            t => Some(t),
        }
    }
    pub fn downloaded(&self) -> u64 {
        self.downloaded.load(Ordering::Relaxed)
    }
    /// 当前正在传输数据的连接数。
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::Relaxed)
    }
    pub fn status(&self) -> Status {
        match self.status.load(Ordering::Relaxed) {
            0 => Status::Probing,
            1 => Status::Downloading,
            2 => Status::Finishing,
            3 => Status::Done,
            4 => Status::Paused,
            _ => Status::Failed,
        }
    }
    /// 是否支持断点续传（服务器支持分段且大小已知）。
    pub fn resumable(&self) -> bool {
        self.resumable.load(Ordering::Relaxed)
    }
    /// 保存位置（最终文件名，下载中实际写入的是同名 `.part`）。
    pub fn path(&self) -> Option<PathBuf> {
        self.path.lock().unwrap().clone()
    }

    pub(crate) fn set_status(&self, s: Status) {
        self.status.store(s as u8, Ordering::Relaxed);
    }
    pub(crate) fn set_total(&self, t: Option<u64>) {
        self.total.store(t.unwrap_or(UNKNOWN), Ordering::Relaxed);
    }
    pub(crate) fn set_downloaded(&self, n: u64) {
        self.downloaded.store(n, Ordering::Relaxed);
    }
    pub(crate) fn add(&self, n: u64) {
        self.downloaded.fetch_add(n, Ordering::Relaxed);
    }
    pub(crate) fn conn_delta(&self, up: bool) {
        if up {
            self.connections.fetch_add(1, Ordering::Relaxed);
        } else {
            self.connections.fetch_sub(1, Ordering::Relaxed);
        }
    }
    pub(crate) fn set_resumable(&self, v: bool) {
        self.resumable.store(v, Ordering::Relaxed);
    }
    pub(crate) fn set_path(&self, p: PathBuf) {
        *self.path.lock().unwrap() = Some(p);
    }
}

pub type SharedProgress = Arc<Progress>;
