//! BT / 磁力链接下载：内核用 librqbit（纯 Rust，支持磁力、DHT、PEX、Tracker、断点续传）。
//!
//! - 不支持 PT（私有种子）：拿到元数据发现是私有种子就停下并提示，不下载。
//! - 我们不伪装成别的客户端，如实报告自己的名字。
//! - 下载完成后从会话里移除（不长期做种）；做种管理以后单独做。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, DhtSessionConfig, ListenerMode, ListenerOptions, ManagedTorrent, Session,
    SessionOptions,
};
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

/// 网络相关的选项（提速用）。
#[derive(Debug, Clone, Default)]
pub struct NetOpts {
    /// 监听端口：让别人能主动连进来（同时开 UPnP 让路由器转发）。None = 不监听。
    pub listen_port: Option<u16>,
    /// 同时用 uTP（UDP 上的 BT 协议）。
    pub utp: bool,
    /// 不开 DHT（防火墙没放行时，避免 Windows 弹窗）。
    pub no_dht: bool,
    /// 连接用户的超时（秒），None = librqbit 默认 10 秒。
    pub connect_timeout: Option<u64>,
    /// 额外使用的公共 Tracker。
    pub trackers: Vec<String>,
}

/// 私有种子（PT）的错误标记，`friendly` 认它。
const PRIVATE_TORRENT: &str = "PANDA_PRIVATE_TORRENT";

/// 每个种子最多同时连接的用户数（librqbit 默认 128）。
const PEER_LIMIT: usize = 250;

impl BtEngine {
    pub async fn new(default_dir: PathBuf, download_limit: u64, net: NetOpts) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&default_dir).ok();
        let trackers: std::collections::HashSet<url::Url> = net.trackers.iter().filter_map(|t| url::Url::parse(t).ok()).collect();
        let make = |dht_cache: bool, port: Option<u16>| {
            let mut opts = SessionOptions::default();
            opts.peer_limit = Some(PEER_LIMIT);
            if let Some(t) = net.connect_timeout {
                opts.connect = Some(librqbit::ConnectionOptions {
                    peer_opts: Some(librqbit::PeerConnectionOptions {
                        connect_timeout: Some(std::time::Duration::from_secs(t)),
                        ..Default::default()
                    }),
                    ..Default::default()
                });
            }
            opts.trackers = trackers.clone();
            if let Some(port) = port {
                opts.listen = Some(ListenerOptions {
                    mode: if net.utp { ListenerMode::TcpAndUtp } else { ListenerMode::TcpOnly },
                    listen_addr: (std::net::Ipv6Addr::UNSPECIFIED, port).into(),
                    enable_upnp_port_forwarding: true,
                    ..Default::default()
                });
            }
            // 我们自己管理"重启后接着下"（重新添加种子，librqbit 会校验已下载的部分）
            opts.persistence = None;
            opts.fastresume = false;
            opts.client_name_and_version = Some(format!("PandaDownloader/{} (rqbit)", env!("CARGO_PKG_VERSION")));
            if download_limit > 0 {
                opts.ratelimits.download_bps = std::num::NonZeroU32::new(download_limit.min(u32::MAX as u64) as u32);
            }
            if net.no_dht {
                opts.dht = None;
            } else if !dht_cache {
                opts.dht = Some(DhtSessionConfig { persistence: None, ..Default::default() });
            }
            opts
        };
        // 依次退让：DHT 节点缓存文件被别的程序占着就不用缓存；端口被占用就换随机端口；还不行就不监听
        let mut session = None;
        let mut last_err = None;
        for (cache, port) in [(true, net.listen_port), (false, net.listen_port), (false, net.listen_port.map(|_| 0)), (false, None)] {
            match Session::new_with_opts(default_dir.clone(), make(cache, port)).await {
                Ok(s) => {
                    session = Some(s);
                    break;
                }
                Err(e) => last_err = Some(e),
            }
        }
        let Some(session) = session else { return Err(last_err.unwrap()) };
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
        // 不做 PT：私有种子直接拒绝（还没开始下载，不会连 PT 站的 Tracker）
        if listed.info.info().private {
            anyhow::bail!("{PRIVATE_TORRENT}");
        }
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

/// 把用户给的地址变成种子来源：磁力链接直接用；网址下载种子文件（带浏览器 Cookie，有的网站要登录）；本地路径读文件。
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

/// 内置的公共 Tracker（联网更新失败时用）。来源：github.com/ngosang/trackerslist。
const BUILTIN_TRACKERS: &str = "\
udp://tracker.opentrackr.org:1337/announce
udp://open.demonii.com:1337/announce
udp://open.stealth.si:80/announce
udp://tracker.torrent.eu.org:451/announce
udp://exodus.desync.com:6969/announce
udp://tracker.theoks.net:6969/announce
udp://explodie.org:6969/announce
udp://tracker.dler.org:6969/announce
udp://tracker.qu.ax:6969/announce
udp://tracker.srv00.com:6969/announce
udp://tracker.filemail.com:6969/announce
udp://tracker.tiny-vps.com:6969/announce
udp://opentracker.io:6969/announce
udp://p4p.arenabg.com:1337/announce
udp://tracker.0x7c0.com:6969/announce
udp://tracker.bittor.pw:1337/announce
http://tracker.opentrackr.org:1337/announce
https://tracker.tamersunion.org:443/announce
";

/// 列表最多用这么多个（太多反而拖慢启动）。
const MAX_TRACKERS: usize = 60;

fn parse_trackers(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in text.lines().map(str::trim) {
        let ok = (l.starts_with("udp://") || l.starts_with("http://") || l.starts_with("https://")) && url::Url::parse(l).is_ok();
        if ok && !out.iter().any(|x| x == l) {
            out.push(l.to_string());
        }
        if out.len() >= MAX_TRACKERS {
            break;
        }
    }
    out
}

pub fn builtin_trackers() -> Vec<String> {
    parse_trackers(BUILTIN_TRACKERS)
}

/// 公共 Tracker：缓存（每天联网更新一次）加内置的，合并去重。
pub fn load_trackers(cache: &Path) -> Vec<String> {
    let mut all = std::fs::read_to_string(cache).map(|t| parse_trackers(&t)).unwrap_or_default();
    for t in builtin_trackers() {
        if all.len() >= MAX_TRACKERS {
            break;
        }
        if !all.contains(&t) {
            all.push(t);
        }
    }
    all
}

/// 缓存超过一天就在后台更新（国内优先走 jsDelivr 镜像）。
pub async fn refresh_trackers(cache: PathBuf) {
    let fresh = std::fs::metadata(&cache)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|d| d < std::time::Duration::from_secs(24 * 3600));
    if fresh {
        return;
    }
    let Ok(client) = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).build() else { return };
    for url in [
        "https://cdn.jsdelivr.net/gh/ngosang/trackerslist@master/trackers_best.txt",
        "https://raw.githubusercontent.com/ngosang/trackerslist/master/trackers_best.txt",
        "https://ngosang.github.io/trackerslist/trackers_best.txt",
    ] {
        let Ok(r) = client.get(url).send().await else { continue };
        let Ok(text) = r.text().await else { continue };
        if parse_trackers(&text).len() >= 5 {
            if let Some(d) = cache.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&cache, text);
            return;
        }
    }
}

/// 把 librqbit 的英文错误换成用户能看懂的话（附原文）。
pub fn friendly(e: &str) -> String {
    if e.contains(PRIVATE_TORRENT) {
        return "这是 PT 站的私有种子，熊猫下载器不支持 PT 下载".into();
    }
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

/// 测速（开发用）：环境变量 `PANDA_BT_BENCH="名字=地址;..."`，可选 `PANDA_BT_SECS`（默认 45）、`PANDA_BT_PORT`、
/// `PANDA_BT_UTP`、`PANDA_BT_TRACKERS`、`PANDA_BT_CT`（连接超时秒数）。每个种子报元数据时间、用户数、平均速度。
pub async fn bench(out: &mut dyn FnMut(String)) {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let Some(list) = env("PANDA_BT_BENCH") else {
        out("没有设置 PANDA_BT_BENCH".into());
        return;
    };
    let secs: u64 = env("PANDA_BT_SECS").and_then(|s| s.parse().ok()).unwrap_or(45);
    let dir = std::env::temp_dir().join("panda_bt_bench");
    let _ = std::fs::remove_dir_all(&dir);
    let net = NetOpts {
        listen_port: env("PANDA_BT_PORT").and_then(|p| p.parse().ok()),
        utp: env("PANDA_BT_UTP").is_some(),
        no_dht: false,
        connect_timeout: env("PANDA_BT_CT").and_then(|p| p.parse().ok()),
        trackers: if env("PANDA_BT_TRACKERS").is_some() { builtin_trackers() } else { vec![] },
    };
    out(format!("BENCH 选项 监听 {:?} uTP {} 超时 {:?} Tracker {} 个", net.listen_port, net.utp, net.connect_timeout, net.trackers.len()));
    let engine = match BtEngine::new(dir.clone(), 0, net).await {
        Ok(e) => e,
        Err(e) => return out(format!("引擎启动失败 {e:#}")),
    };
    for item in list.split(';').filter(|s| !s.is_empty()) {
        let Some((name, loc)) = item.split_once('=') else { continue };
        let src = match fetch_source(loc, &ReqCtx::default()).await {
            Ok(s) => s,
            Err(e) => {
                out(format!("BENCH {name}: {e}"));
                continue;
            }
        };
        let t0 = std::time::Instant::now();
        let added = tokio::time::timeout(std::time::Duration::from_secs(90), engine.add(&src, &dir, false)).await;
        let Ok(Ok((h, _))) = added else {
            out(format!("BENCH {name}: 90 秒内没拿到元数据"));
            continue;
        };
        let meta_t = t0.elapsed().as_secs_f64();
        let t1 = std::time::Instant::now();
        let (mut max_peers, mut peers_10, mut best) = (0, 0, 0u64);
        let mut last = 0u64;
        for i in 1..=secs {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let l = live_of(&h);
            max_peers = max_peers.max(l.peers);
            best = best.max(l.downloaded.saturating_sub(last));
            last = l.downloaded;
            if i == 10 {
                peers_10 = l.peers;
            }
            if l.finished {
                break;
            }
        }
        let got = live_of(&h).downloaded;
        let el = t1.elapsed().as_secs_f64();
        out(format!(
            "BENCH {name}: 元数据 {meta_t:.1}s | 10 秒时 {peers_10} 个用户 | 最多 {max_peers} 个 | {:.1} MB / {el:.0}s = 平均 {:.2} MB/s，最快 {:.2} MB/s",
            got as f64 / 1048576.0,
            got as f64 / 1048576.0 / el,
            best as f64 / 1048576.0
        ));
        engine.remove(&h, true).await;
    }
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

    /// PT 私有种子：拿到元数据就拒绝，提示不支持。
    #[tokio::test(flavor = "multi_thread")]
    async fn rejects_private() {
        let mut b = b"d4:infod6:lengthi1e4:name1:a12:piece lengthi16384e6:pieces20:".to_vec();
        b.extend_from_slice(&[7u8; 20]);
        b.extend_from_slice(b"7:privatei1eee");
        let dir = std::env::temp_dir().join("panda_bt_private");
        let engine = BtEngine::new(dir.clone(), 0, NetOpts { no_dht: true, ..Default::default() }).await.unwrap();
        let err = engine.add(&Source::Bytes(Bytes::from(b)), &dir, false).await.err().expect("应该拒绝");
        assert!(friendly(&format!("{err:#}")).contains("不支持 PT"));
        assert!(!dir.join("a").exists());
        // 同样的种子去掉 private 就能加
        let mut ok = b"d4:infod6:lengthi1e4:name1:a12:piece lengthi16384e6:pieces20:".to_vec();
        ok.extend_from_slice(&[7u8; 20]);
        ok.extend_from_slice(b"ee");
        let (h, meta) = engine.add(&Source::Bytes(Bytes::from(ok)), &dir, true).await.unwrap();
        assert_eq!(meta.name, "a");
        engine.remove(&h, true).await;
    }

    #[test]
    fn trackers() {
        let b = builtin_trackers();
        assert!(b.len() >= 15 && b.iter().all(|t| !t.starts_with("wss")));
        let t = parse_trackers("udp://a.com:1/announce

wss://x.com/a
udp://a.com:1/announce
http://b.com/announce
垃圾");
        assert_eq!(t, vec!["udp://a.com:1/announce", "http://b.com/announce"]);
        // 没有缓存时用内置的
        assert_eq!(load_trackers(Path::new("Z:/不存在/trackers.txt")), b);
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
        let engine = BtEngine::new(dir.clone(), 0, NetOpts::default()).await.unwrap();
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

    /// 测速：`PANDA_BT_BENCH="名字=地址;名字=地址" cargo test --release -p panda-app bt_bench -- --ignored --nocapture`
    /// 要测监听端口 / uTP（会触发防火墙弹窗）时改用 `PandaDownloader.exe --bt-bench <结果文件>`。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn bt_bench() {
        bench(&mut |l| println!("{l}")).await;
    }
}
