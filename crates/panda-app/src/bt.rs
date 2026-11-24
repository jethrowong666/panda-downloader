//! BT / 磁力链接下载：内核用 librqbit（纯 Rust，支持磁力、DHT、PEX、Tracker、断点续传）。
//!
//! - 私有种子（PT）由 librqbit 自动处理：不开 DHT、不做局域网发现，只用种子里的 Tracker。
//! - 我们不伪装成别的客户端，如实报告自己的名字。
//! - 下载完成后从会话里移除（不长期做种）；做种管理以后单独做。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse, DhtSessionConfig, ManagedTorrent, Session, SessionOptions};
use serde::{Deserialize, Serialize};

use crate::bridge::ReqCtx;

pub type Handle = Arc<ManagedTorrent>;

pub struct BtEngine {
    session: Arc<Session>,
    default_dir: PathBuf,
}

/// 种子来源。
#[derive(Debug, Clone)]
pub enum Source {
    Magnet(String),
    /// 种子文件内容（网址下载来的、本地文件、或者上次保存的）。
    Bytes(Bytes),
}

/// 种子的基本信息（拿到元数据之后才有）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Meta {
    pub name: String,
    pub total: u64,
    pub files: usize,
    pub info_hash: String,
    pub private: bool,
    /// 下载内容相对保存位置的路径：多文件种子是文件夹，单文件种子是那个文件。
    pub content: PathBuf,
}

/// 实时状态。
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
pub struct Live {
    pub downloaded: u64,
    pub total: u64,
    pub finished: bool,
    pub peers: u32,
    pub upload_speed: u64,
    pub error: Option<String>,
}

impl BtEngine {
    pub async fn new(default_dir: PathBuf, download_limit: u64) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&default_dir).ok();
        let make = |dht_cache: bool| {
            let mut opts = SessionOptions::default();
            // 我们自己管理"重启后接着下"（重新添加种子，librqbit 会校验已下载的部分）
            opts.persistence = None;
            opts.fastresume = false;
            opts.client_name_and_version = Some(format!("PandaDownloader/{} (rqbit)", env!("CARGO_PKG_VERSION")));
            if download_limit > 0 {
                opts.ratelimits.download_bps = std::num::NonZeroU32::new(download_limit.min(u32::MAX as u64) as u32);
            }
            if !dht_cache {
                opts.dht = Some(DhtSessionConfig { persistence: None, ..Default::default() });
            }
            opts
        };
        // DHT 节点缓存文件被别的程序占着（比如另一个 rqbit）时，不用缓存再试一次
        let session = match Session::new_with_opts(default_dir.clone(), make(true)).await {
            Ok(s) => s,
            Err(_) => Session::new_with_opts(default_dir.clone(), make(false)).await?,
        };
        Ok(Self { session, default_dir })
    }

    /// 添加种子。`paused` = 只拿元数据先不下载（确认窗还没确认时用）。
    pub async fn add(&self, source: &Source, out_dir: &Path, paused: bool) -> anyhow::Result<(Handle, Meta)> {
        std::fs::create_dir_all(out_dir)?;
        let add = match source {
            Source::Magnet(m) => AddTorrent::from_url(m.clone()),
            Source::Bytes(b) => AddTorrent::from_bytes(b.clone()),
        };
        // 第一步只拿元数据（磁力链接要上网找），算出多文件种子该放进哪个子文件夹。
        // librqbit 指定了保存位置就不再自动建子文件夹，所以这一步我们自己来。
        let probe = AddTorrentOptions { list_only: true, ..Default::default() };
        let listed = match self.session.add_torrent(add, Some(probe)).await? {
            AddTorrentResponse::ListOnly(l) => l,
            AddTorrentResponse::Added(_, h) | AddTorrentResponse::AlreadyManaged(_, h) => {
                let meta = meta_of(&h);
                return Ok((h, meta));
            }
        };
        let sub = listed
            .output_folder
            .strip_prefix(&self.default_dir)
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        // 文件夹名去掉 Windows 不允许的字符
        let sub: PathBuf = sub.components().map(|c| panda_http::filename::sanitize(&c.as_os_str().to_string_lossy())).collect();
        let target = out_dir.join(&sub);
        std::fs::create_dir_all(&target)?;
        let opts = AddTorrentOptions {
            paused,
            // 允许在已有文件上继续（断点续传、换目录后重新添加）
            overwrite: true,
            output_folder: Some(target.to_string_lossy().into_owned()),
            initial_peers: Some(listed.seen_peers.clone()).filter(|p| !p.is_empty()),
            ..Default::default()
        };
        let handle = match self.session.add_torrent(AddTorrent::from_bytes(listed.torrent_bytes.clone()), Some(opts)).await? {
            AddTorrentResponse::Added(_, h) | AddTorrentResponse::AlreadyManaged(_, h) => h,
            AddTorrentResponse::ListOnly(_) => anyhow::bail!("种子只列出了文件，没有开始"),
        };
        let mut meta = meta_of(&handle);
        if meta.files == 1 {
            meta.content = handle.with_metadata(|m| m.file_infos[0].relative_filename.clone()).unwrap_or_default();
        } else {
            meta.content = sub;
        }
        Ok((handle, meta))
    }

    pub async fn pause(&self, h: &Handle) {
        let _ = self.session.pause(h).await;
    }

    pub async fn resume(&self, h: &Handle) -> anyhow::Result<()> {
        self.session.unpause(h).await
    }

    /// 从会话里移除；`delete_files` 为真时连已下载的文件一起删。
    pub async fn remove(&self, h: &Handle, delete_files: bool) {
        let _ = self.session.delete(h.id().into(), delete_files).await;
    }
}

pub fn meta_of(h: &Handle) -> Meta {
    let (name, files, private, total) = h
        .with_metadata(|m| {
            let name = m.info.name().map(|n| n.to_string()).unwrap_or_default();
            let total: u64 = m.file_infos.iter().map(|f| f.len).sum();
            (name, m.file_infos.len(), m.info.info().private, total)
        })
        .unwrap_or_default();
    let name = if name.is_empty() { h.name().unwrap_or_default() } else { name };
    let content = PathBuf::from(panda_http::filename::sanitize(&name));
    Meta { name, total, files, info_hash: h.info_hash().as_string(), private, content }
}

pub fn live_of(h: &Handle) -> Live {
    let s = h.stats();
    let (peers, up) = match &s.live {
        Some(l) => (l.snapshot.peer_stats.live, l.upload_speed.as_bytes()),
        None => (0, 0),
    };
    Live { downloaded: s.progress_bytes, total: s.total_bytes, finished: s.finished, peers, upload_speed: up, error: s.error.clone() }
}

/// 种子文件的完整内容（磁力链接拿到元数据后保存下来，重启时不用再去网络上找）。
pub fn torrent_bytes(h: &Handle) -> Option<Bytes> {
    h.with_metadata(|m| m.torrent_bytes.clone()).ok().filter(|b| !b.is_empty())
}

/// 下载完成后的路径：单文件种子是文件，多文件是文件夹。
pub fn output_path(meta: &Meta, out_dir: &Path) -> PathBuf {
    out_dir.join(&meta.content)
}

/// 把用户给的地址变成种子来源：磁力链接直接用；网址下载种子文件（带浏览器 Cookie，PT 站常要）；本地路径读文件。
pub async fn fetch_source(location: &str, ctx: &ReqCtx) -> Result<Source, String> {
    let loc = location.trim();
    if loc.to_ascii_lowercase().starts_with("magnet:") {
        return Ok(Source::Magnet(loc.to_string()));
    }
    // 裸 info hash
    if loc.len() == 40 && loc.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(Source::Magnet(format!("magnet:?xt=urn:btih:{loc}")));
    }
    if loc.starts_with("http://") || loc.starts_with("https://") {
        let mut b = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30));
        if crate::video::prefers_direct(loc) {
            b = b.no_proxy();
        }
        let client = b.build().map_err(|e| e.to_string())?;
        let mut req = client.get(loc);
        if let Some(r) = &ctx.referer {
            req = req.header("Referer", r);
        }
        // 有的网站不给没有 User-Agent 的请求（比如 archive.org 返回 403）
        let ua = ctx.user_agent.as_deref().unwrap_or("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0 Safari/537.36");
        req = req.header("User-Agent", ua);
        if let Some(c) = ctx.cookie_header(loc) {
            req = req.header("Cookie", c);
        }
        let resp = req.send().await.map_err(|e| format!("下载种子文件失败：{e}"))?;
        if !resp.status().is_success() {
            return Err(format!("下载种子文件失败：服务器返回 {}", resp.status().as_u16()));
        }
        let bytes = resp.bytes().await.map_err(|e| format!("下载种子文件失败：{e}"))?;
        return check_torrent(bytes);
    }
    let path = loc.trim_start_matches("file:///");
    let data = std::fs::read(path).map_err(|e| format!("读不了种子文件 {path}：{e}"))?;
    check_torrent(Bytes::from(data))
}

/// 种子文件是 bencode 字典，以 'd' 开头；下到的是网页（比如要登录）就提示。
fn check_torrent(b: Bytes) -> Result<Source, String> {
    if b.first() == Some(&b'd') {
        Ok(Source::Bytes(b))
    } else if b.starts_with(b"<") || b.windows(5).take(512).any(|w| w.eq_ignore_ascii_case(b"<html")) {
        Err("拿到的是网页而不是种子文件，可能需要先在浏览器里登录这个网站".into())
    } else {
        Err("这不是有效的种子文件".into())
    }
}

/// 把 librqbit 的英文错误换成用户能看懂的话（附原文）。
pub fn friendly(e: &str) -> String {
    let low = e.to_ascii_lowercase();
    let msg = if low.contains("timeout") || low.contains("timed out") {
        "找不到能连接的用户（磁力链接可能已经没人分享了）"
    } else if low.contains("invalid") && low.contains("magnet") {
        "磁力链接格式不对"
    } else if low.contains("no space") || low.contains("disk full") {
        "磁盘空间不够"
    } else if low.contains("permission") || low.contains("access is denied") {
        "没有权限写入保存位置"
    } else {
        "BT 下载出错"
    };
    let raw: String = e.chars().take(200).collect();
    format!("{msg}\n详情：{raw}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torrent_check() {
        assert!(matches!(check_torrent(Bytes::from_static(b"d8:announce3:abce")), Ok(Source::Bytes(_))));
        assert!(check_torrent(Bytes::from_static(b"<!doctype html><html>login</html>")).unwrap_err().contains("登录"));
        assert!(check_torrent(Bytes::from_static(b"garbage")).is_err());
    }

    #[tokio::test]
    async fn sources() {
        let ctx = ReqCtx::default();
        assert!(matches!(fetch_source("magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a", &ctx).await, Ok(Source::Magnet(_))));
        match fetch_source("c12fe1c06bba254a9dc9f519b335aa7c1367a88a", &ctx).await {
            Ok(Source::Magnet(m)) => assert!(m.starts_with("magnet:?xt=urn:btih:")),
            other => panic!("{other:?}"),
        }
        assert!(fetch_source("Z:\\不存在\\x.torrent", &ctx).await.is_err());
    }

    /// 联网实测：`PANDA_BT_TEST=<磁力链接或种子网址> cargo test -p panda-app real_torrent -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn real_torrent() {
        let loc = std::env::var("PANDA_BT_TEST").expect("PANDA_BT_TEST");
        let dir = std::env::temp_dir().join("panda_bt_test");
        let _ = std::fs::remove_dir_all(&dir);
        let engine = BtEngine::new(dir.clone(), 0).await.unwrap();
        let src = fetch_source(&loc, &ReqCtx::default()).await.unwrap();
        let t0 = std::time::Instant::now();
        let (h, meta) = tokio::time::timeout(std::time::Duration::from_secs(120), engine.add(&src, &dir, false)).await.expect("元数据超时").unwrap();
        println!("元数据 {:?} 用时 {:?}", meta, t0.elapsed());
        assert!(meta.total > 0 && !meta.name.is_empty());
        assert!(torrent_bytes(&h).is_some());
        let mut last = 0;
        for i in 0..60 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let l = live_of(&h);
            println!("{}s 已下 {} / {}，{} 个用户 {:?}", i * 2, l.downloaded, l.total, l.peers, l.error);
            last = l.downloaded;
            if l.finished || l.downloaded > 8 << 20 {
                break;
            }
        }
        assert!(last > 0, "一个字节都没下到");
        engine.pause(&h).await;
        engine.resume(&h).await.unwrap();
        let out = output_path(&meta, &dir);
        println!("输出位置 {}", out.display());
        assert!(out.exists());
        engine.remove(&h, true).await;
    }
}
