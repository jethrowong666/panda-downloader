//! 任务管理：链接捕获 → 预下载 → 确认（可改名/换目录/选清晰度）→ 下载 → 完成处理。
//!
//! 两种任务：
//! - 普通文件：直接交给 HTTP 引擎。
//! - 视频网站：yt-dlp 解析出各清晰度的真实地址，视频、音频两路用 HTTP 引擎并行下载，最后 ffmpeg 合并。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use panda_http::state::{part_path, state_path};
use panda_http::{ClientConfig, DownloadError, Downloader, PauseHandle, Progress, RateLimiter, Status, TaskOptions};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;

use crate::bridge::{CaptureItem, ReqCtx};
use crate::category::{self, Action, Category};
use crate::store::{self, History, HistoryItem, SavedBt, SavedTask, SavedVideo, Settings};
use crate::bt;
use crate::video::{self, VideoInfo};
use crate::{sys, windows};

/// 取消后多久内可以撤销。
const UNDO_WINDOW: Duration = Duration::from_secs(5);
/// 视频地址通常一两个小时后过期，超过这个时间就重新解析。
const VIDEO_INFO_TTL: Duration = Duration::from_secs(30 * 60);
/// 视频网站对单个连接数比较敏感，用少一点。
const VIDEO_CONNECTIONS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum St {
    Downloading,
    Paused,
    Done,
    Failed,
    Cancelled,
}

/// 视频的某一路下载失败的原因。
enum PartErr {
    Cancelled,
    /// 地址过期，需要重新解析
    Expired,
    Msg(String),
}

/// 视频任务的附加状态。
struct VideoJob {
    page_url: String,
    info: Option<VideoInfo>,
    resolved_at: Option<Instant>,
    resolving: bool,
    choice: Option<String>,
    tmp_dir: PathBuf,
    merging: bool,
    /// 当前这次运行的各路下载进度（视频、音频）。
    parts: Arc<Mutex<Vec<(Arc<Progress>, Option<u64>)>>>,
    pauses: Arc<Mutex<Vec<PauseHandle>>>,
    cancel: CancellationToken,
}

impl VideoJob {
    fn new(page_url: String, tmp_dir: PathBuf, choice: Option<String>) -> Self {
        Self {
            page_url,
            info: None,
            resolved_at: None,
            resolving: false,
            choice,
            tmp_dir,
            merging: false,
            parts: Arc::default(),
            pauses: Arc::default(),
            cancel: CancellationToken::new(),
        }
    }

    fn chosen(&self) -> Option<&video::Choice> {
        let info = self.info.as_ref()?;
        self.choice
            .as_deref()
            .and_then(|k| info.choices.iter().find(|c| c.key == k))
            .or_else(|| info.choices.first())
    }

    fn stop(&self) {
        self.cancel.cancel();
        for p in self.pauses.lock().unwrap().iter() {
            p.pause();
        }
    }

    /// (已下载, 总大小)
    fn progress(&self) -> (u64, Option<u64>) {
        let parts = self.parts.lock().unwrap();
        if parts.is_empty() {
            return (0, self.chosen().and_then(|c| c.size));
        }
        let done = parts.iter().map(|(p, _)| p.downloaded()).sum();
        let mut total = 0;
        for (p, est) in parts.iter() {
            match p.total().or(*est) {
                Some(t) => total += t,
                None => return (done, None),
            }
        }
        (done, Some(total))
    }
}

/// BT / 磁力任务的附加状态。
struct BtJob {
    /// 用户给的地址：磁力链接、种子网址、本地种子路径或 info hash。
    location: String,
    /// 拿到元数据后保存的种子文件（重启后直接用，不用再去网络上找）。
    saved_torrent: Option<PathBuf>,
    engine: Option<Arc<bt::BtEngine>>,
    handle: Option<bt::Handle>,
    meta: Option<bt::Meta>,
    resolving: bool,
    /// 重启后种子还没加回会话时显示的已下载量。
    last_downloaded: u64,
    cancel: CancellationToken,
}

impl BtJob {
    fn new(location: String, saved_torrent: Option<PathBuf>) -> Self {
        Self { location, saved_torrent, engine: None, handle: None, meta: None, resolving: false, last_downloaded: 0, cancel: CancellationToken::new() }
    }

    /// 已下载字节数：种子在会话里就取实时值，否则用上次记下的。
    fn downloaded(&self) -> u64 {
        self.live().map(|l| l.downloaded).unwrap_or(self.last_downloaded)
    }

    fn live(&self) -> Option<bt::Live> {
        self.handle.as_ref().map(bt::live_of)
    }

    /// 停下：等待中的流程退出，种子暂停（保留已下载的部分）。
    fn stop(&self) {
        self.cancel.cancel();
        if let (Some(e), Some(h)) = (self.engine.clone(), self.handle.clone()) {
            tauri::async_runtime::spawn(async move { e.pause(&h).await });
        }
    }

    /// 彻底移除：从 BT 会话里删掉，连同已下载的文件和保存的种子文件。
    fn discard(&self, dir: &Path) {
        self.cancel.cancel();
        if let (Some(e), Some(h)) = (self.engine.clone(), self.handle.clone()) {
            tauri::async_runtime::spawn(async move { e.remove(&h, true).await });
        } else if let Some(m) = self.meta.as_ref() {
            // 重启后还没加回会话的种子：直接删它的文件 / 文件夹
            let ok = !m.content.as_os_str().is_empty()
                && m.content.components().all(|c| matches!(c, std::path::Component::Normal(_)));
            if ok {
                let p = dir.join(&m.content);
                if p.is_dir() {
                    let _ = std::fs::remove_dir_all(&p);
                } else {
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
        if let Some(p) = &self.saved_torrent {
            let _ = std::fs::remove_file(p);
        }
    }
}

pub struct Entry {
    id: u64,
    group: u64,
    link: String,
    /// 解包后的 http(s) 地址；不支持的链接为空。
    url: Option<String>,
    unsupported: Option<String>,
    dir: PathBuf,
    /// 用户指定的文件名；为空时由服务器响应或视频标题决定。
    name: Option<String>,
    status: St,
    confirmed: bool,
    progress: Option<Arc<Progress>>,
    pause: Option<PauseHandle>,
    run_gen: u64,
    error: Option<String>,
    final_path: Option<PathBuf>,
    dup_of: Option<PathBuf>,
    speed: f64,
    last_sample: Option<(Instant, u64)>,
    cancel_at: Option<Instant>,
    done_dismissed: bool,
    relocating: bool,
    video: Option<VideoJob>,
    bt: Option<BtJob>,
    /// 浏览器带来的来源页、Cookie、UA（只在内存里）。
    ctx: ReqCtx,
}

impl Entry {
    fn new(id: u64, group: u64, link: String, url: Option<String>, dir: PathBuf, name: Option<String>) -> Self {
        Self {
            id,
            group,
            link,
            url,
            unsupported: None,
            dir,
            name,
            status: St::Paused,
            confirmed: false,
            progress: None,
            pause: None,
            run_gen: 0,
            error: None,
            final_path: None,
            dup_of: None,
            speed: 0.0,
            last_sample: None,
            cancel_at: None,
            done_dismissed: false,
            relocating: false,
            video: None,
            bt: None,
            ctx: ReqCtx::default(),
        }
    }

    fn display_name(&self) -> String {
        if let Some(p) = &self.final_path {
            if let Some(n) = p.file_name() {
                return n.to_string_lossy().into_owned();
            }
        }
        if let Some(n) = &self.name {
            return n.clone();
        }
        if let Some(b) = &self.bt {
            return match &b.meta {
                Some(m) if !m.name.is_empty() => m.name.clone(),
                _ => "正在获取种子信息…".into(),
            };
        }
        if let Some(v) = &self.video {
            return match (&v.info, v.chosen()) {
                (Some(info), Some(c)) => format!("{}.{}", panda_http::filename::sanitize(&info.title), c.container),
                _ => "正在解析视频…".into(),
            };
        }
        if let Some(p) = self.progress.as_ref().and_then(|p| p.path()) {
            if let Some(n) = p.file_name() {
                return n.to_string_lossy().into_owned();
            }
        }
        self.url.as_deref().and_then(category::name_from_url).unwrap_or_else(|| "下载".into())
    }

    /// 当前对应的最终文件路径（下载中实际写的是同名 .part）。
    fn current_path(&self) -> Option<PathBuf> {
        if self.video.is_some() {
            return self.final_path.clone();
        }
        if let Some(b) = &self.bt {
            return self.final_path.clone().or_else(|| {
                b.meta.as_ref().filter(|m| !m.name.is_empty()).map(|m| bt::output_path(m, &self.dir))
            });
        }
        self.final_path.clone().or_else(|| self.progress.as_ref().and_then(|p| p.path()))
    }

    fn is_active(&self) -> bool {
        self.confirmed && matches!(self.status, St::Downloading | St::Paused | St::Failed | St::Cancelled)
    }

    fn downloaded(&self) -> u64 {
        if let Some(b) = &self.bt {
            return b.downloaded();
        }
        match &self.video {
            Some(v) => v.progress().0,
            None => self.progress.as_ref().map_or(0, |p| p.downloaded()),
        }
    }

    /// 停下当前这次运行（暂停 / 取消 / 换清晰度时）。
    fn stop(&self) {
        if let Some(p) = &self.pause {
            p.pause();
        }
        if let Some(v) = &self.video {
            v.stop();
        }
        if let Some(b) = &self.bt {
            b.stop();
        }
    }
}

/// 给界面看的任务快照。
#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    pub id: u64,
    pub group: u64,
    pub name: String,
    pub dir: String,
    pub path: Option<String>,
    pub url: String,
    pub total: Option<u64>,
    pub downloaded: u64,
    pub speed: u64,
    pub status: St,
    pub confirmed: bool,
    pub error: Option<String>,
    pub resumable: bool,
    pub probing: bool,
    pub category: &'static str,
    pub action: Action,
    pub action_label: &'static str,
    pub dup_of: Option<String>,
    pub unsupported: Option<String>,
    pub undo_ms: Option<u64>,
    pub free_space: Option<u64>,
    /// 完成窗里已经关掉了。
    pub dismissed: bool,
    pub video: Option<VideoView>,
    pub bt: Option<BtView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BtView {
    /// 正在获取种子信息（磁力链接找元数据、下载种子文件）。
    pub resolving: bool,
    pub peers: u32,
    pub files: usize,
    pub private: bool,
    pub upload_speed: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct VideoView {
    pub resolving: bool,
    pub merging: bool,
    pub title: Option<String>,
    pub thumbnail: Option<String>,
    pub duration: Option<f64>,
    pub uploader: Option<String>,
    pub site: Option<String>,
    pub choice: Option<String>,
    pub choices: Vec<ChoiceView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChoiceView {
    pub key: String,
    pub label: String,
    pub detail: String,
    pub size: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ConfirmItem {
    pub id: u64,
    pub name: String,
    pub dir: String,
}

struct Inner {
    next_id: u64,
    next_group: u64,
    tasks: Vec<Entry>,
    settings: Settings,
    history: History,
    /// 等待弹出确认窗的任务组（用户全屏时先排队）。
    pending_confirm: Vec<u64>,
    dirty: bool,
}

pub struct Manager {
    app: AppHandle,
    dl: Downloader,
    /// 国内网站直接连接，不走系统代理。
    dl_direct: Downloader,
    limiter: Arc<RateLimiter>,
    data_dir: PathBuf,
    inner: Mutex<Inner>,
    /// BT 引擎：第一次有 BT 任务时才启动（会监听端口、连 DHT）。
    bt_engine: tokio::sync::OnceCell<Arc<bt::BtEngine>>,
}

fn friendly(e: &DownloadError) -> String {
    match e {
        DownloadError::Status(401 | 403 | 410) => "链接已过期或需要登录，回到网页重新点一次下载".into(),
        DownloadError::Status(404) => "服务器上找不到这个文件（404）".into(),
        DownloadError::Status(c) => format!("服务器拒绝了请求（HTTP {c}）"),
        DownloadError::RetriesExhausted { .. } | DownloadError::Network(_) | DownloadError::Timeout(_) => {
            "网络连接不上，已自动重试多次".into()
        }
        DownloadError::RemoteChanged => "服务器上的文件变了，需要重新下载".into(),
        DownloadError::Io(err) => format!("写入硬盘失败：{err}"),
        other => other.to_string(),
    }
}

/// 视频的临时文件夹：放在保存位置下，按页面地址区分，重启后还能找到。
fn video_tmp_dir(dir: &Path, page_url: &str) -> PathBuf {
    // FNV-1a，够用来区分不同视频
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in page_url.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    dir.join(".熊猫下载临时文件").join(format!("{h:016x}"))
}

impl Manager {
    pub fn new(app: AppHandle, data_dir: PathBuf) -> Arc<Self> {
        let settings: Settings = store::load(&data_dir.join("settings.json"));
        let history: History = store::load(&data_dir.join("history.json"));
        let limiter = Arc::new(RateLimiter::new(settings.speed_limit));
        let dl = Downloader::with_limiter(&ClientConfig::default(), limiter.clone()).expect("创建下载器失败");
        let dl_direct = Downloader::with_limiter(&ClientConfig { direct: true, ..Default::default() }, limiter.clone())
            .expect("创建下载器失败");
        Arc::new(Self {
            app,
            dl,
            dl_direct,
            limiter,
            data_dir,
            inner: Mutex::new(Inner {
                next_id: 1,
                next_group: 1,
                tasks: Vec::new(),
                settings,
                history,
                pending_confirm: Vec::new(),
                dirty: false,
            }),
            bt_engine: tokio::sync::OnceCell::new(),
        })
    }

    async fn bt_engine(&self) -> Result<Arc<bt::BtEngine>, String> {
        if let Some(e) = self.bt_engine.get() {
            return Ok(e.clone());
        }
        // 固定一个监听端口（第一次随机选，之后一直用这个，路由器映射和对方缓存的地址都不会失效）
        let (base, limit, port) = {
            let mut g = self.inner.lock().unwrap();
            if g.settings.bt_port == 0 {
                let mut b = [0u8; 2];
                let _ = getrandom::fill(&mut b);
                g.settings.bt_port = 20000 + (u16::from_le_bytes(b) % 40000);
                store::save(&self.data_dir.join("settings.json"), &g.settings);
            }
            (g.settings.base_dir.clone(), g.settings.speed_limit, g.settings.bt_port)
        };
        let firewall_ok = tokio::task::spawn_blocking(sys::firewall_has_rule).await.unwrap_or(false);
        let cache = self.data_dir.join("trackers.txt");
        tauri::async_runtime::spawn(bt::refresh_trackers(cache.clone()));
        let net = bt::NetOpts {
            // 防火墙没放行时不监听、不开 DHT，避免 Windows 弹窗
            listen_port: firewall_ok.then_some(port),
            utp: false,
            no_dht: !firewall_ok,
            trackers: bt::load_trackers(&cache),
        };
        self.bt_engine
            .get_or_try_init(|| async move { bt::BtEngine::new(base, limit, net).await.map(Arc::new) })
            .await
            .cloned()
            .map_err(|e| bt::friendly(&format!("BT 引擎启动失败：{e:#}")))
    }

    pub fn settings(&self) -> Settings {
        self.inner.lock().unwrap().settings.clone()
    }

    pub fn set_settings(&self, s: Settings) {
        self.limiter.set_rate(s.speed_limit);
        store::save(&self.data_dir.join("settings.json"), &s);
        self.inner.lock().unwrap().settings = s;
    }

    pub fn save_float_pos(&self, x: i32, y: i32) {
        let mut g = self.inner.lock().unwrap();
        g.settings.float_pos = Some((x, y));
        store::save(&self.data_dir.join("settings.json"), &g.settings);
    }

    /// 启动时恢复上次没下完的任务。
    pub fn restore(self: &Arc<Self>) {
        let saved: Vec<SavedTask> = store::load(&self.data_dir.join("tasks.json"));
        let mut g = self.inner.lock().unwrap();
        let auto = g.settings.auto_resume;
        let conns = g.settings.connections;
        for s in saved {
            let id = g.next_id;
            g.next_id += 1;
            let group = g.next_group;
            g.next_group += 1;
            let mut e = Entry::new(id, group, s.link, Some(s.url), s.dir, s.name);
            if let Some(v) = s.video {
                e.video = Some(VideoJob::new(v.page_url, v.tmp_dir, v.choice));
            }
            if let Some(b) = s.bt {
                let mut job = BtJob::new(b.location, b.torrent_file);
                job.meta = b.meta;
                job.last_downloaded = b.downloaded;
                e.bt = Some(job);
                e.name = None;
            }
            e.confirmed = true;
            e.status = St::Paused;
            if auto && !s.paused {
                self.start(&mut e, conns);
            }
            g.tasks.push(e);
        }
    }

    /// 捕获链接（剪贴板、快捷键、拖放、手动输入）。
    pub fn capture(self: &Arc<Self>, links: Vec<String>) {
        let items = links.into_iter().map(|url| CaptureItem { url, filename: None, video: false }).collect();
        self.capture_items(items, ReqCtx::default());
    }

    /// 捕获链接，附带浏览器扩展给的上下文（来源页、Cookie、建议的文件名）。
    pub fn capture_items(self: &Arc<Self>, items: Vec<CaptureItem>, ctx: ReqCtx) {
        let mut g = self.inner.lock().unwrap();
        let group = g.next_group;
        g.next_group += 1;
        let ask = g.settings.ask_before_download;
        let conns = g.settings.connections;
        let mut need_confirm = false;
        let mut added = 0;
        for item in items {
            let link = item.url.clone();
            let Ok(parsed) = panda_link::parse(&link) else { continue };
            let id = g.next_id;
            // BT：磁力链接、种子网址、本地种子文件；浏览器接管的下载如果文件名是 .torrent 也算
            let is_bt = parsed.engine() == panda_link::Engine::BitTorrent
                || item.filename.as_deref().is_some_and(|f| f.to_ascii_lowercase().ends_with(".torrent"));
            if is_bt {
                let location = parsed.resolved.clone();
                if g.tasks.iter().any(|t| t.bt.as_ref().is_some_and(|b| b.location == location) && matches!(t.status, St::Downloading | St::Paused)) {
                    continue;
                }
                let dir = g.settings.base_dir.join("BT下载");
                let mut e = Entry::new(id, group, link, Some(location.clone()), dir, None);
                e.bt = Some(BtJob::new(location, None));
                e.ctx = ctx.clone();
                e.confirmed = !ask;
                g.next_id += 1;
                // 和普通下载一样：弹确认窗的同时就开始找种子信息、开始下载
                self.start(&mut e, conns);
                g.tasks.push(e);
                need_confirm |= ask;
                added += 1;
                continue;
            }
            if parsed.engine() != panda_link::Engine::Http {
                let mut e = Entry::new(id, group, link, None, g.settings.base_dir.clone(), None);
                e.status = St::Failed;
                e.unsupported = Some(format!("{}链接会在后续版本支持", parsed.engine().label()));
                g.next_id += 1;
                g.tasks.push(e);
                need_confirm = true;
                added += 1;
                continue;
            }
            let url = parsed.resolved.clone();
            // 同一个链接已经在下载了，就不重复添加。
            if g.tasks.iter().any(|t| t.url.as_deref() == Some(&url) && matches!(t.status, St::Downloading | St::Paused)) {
                continue;
            }
            if item.video || video::is_video_page(&url) || video::is_stream_manifest(&url) {
                // 视频网站：先解析，解析完就开始预下载最高画质。
                let dir = g.settings.dir_for(Category::Video);
                let mut e = Entry::new(id, group, link, Some(url.clone()), dir.clone(), None);
                e.video = Some(VideoJob::new(url.clone(), video_tmp_dir(&dir, &url), None));
                e.ctx = ctx.clone();
                e.confirmed = !ask;
                g.next_id += 1;
                self.start(&mut e, conns);
                g.tasks.push(e);
                need_confirm |= ask;
                added += 1;
                continue;
            }
            // 有建议的文件名（浏览器扩展给的）就按它分类，抖音等视频地址本身没有扩展名
            let guess = item.filename.clone().or_else(|| category::name_from_url(&url)).unwrap_or_default();
            let dir = g.settings.dir_for(category::category_of(&guess));
            let name = item.filename.as_deref().map(panda_http::filename::sanitize);
            let mut e = Entry::new(id, group, link, Some(url.clone()), dir, name);
            e.ctx = ctx.clone();
            g.next_id += 1;
            if let Some(h) = g.history.find_existing(&url) {
                // 下载过而且文件还在：先问用户，不预下载。
                e.dup_of = Some(h.path.clone());
                e.status = St::Paused;
                need_confirm = true;
            } else {
                e.confirmed = !ask;
                need_confirm |= ask;
                // 弹窗的同时就开始下载，用户确认时往往已经下了一大截。
                self.start(&mut e, conns);
            }
            g.tasks.push(e);
            added += 1;
        }
        if added > 0 && need_confirm {
            g.pending_confirm.push(group);
        }
        g.dirty = true;
    }

    /// 国内网站（或国内网站的视频）用直连的下载器。
    fn downloader_for(&self, url: &str, page: Option<&str>) -> &Downloader {
        if video::prefers_direct(url) || page.is_some_and(video::prefers_direct) {
            &self.dl_direct
        } else {
            &self.dl
        }
    }

    /// 开始 / 继续一个 BT 任务。
    fn spawn_bt(self: &Arc<Self>, e: &mut Entry) {
        let Some(job) = e.bt.as_mut() else { return };
        job.cancel = CancellationToken::new();
        job.resolving = job.meta.is_none();
        e.run_gen += 1;
        e.status = St::Downloading;
        e.error = None;
        e.speed = 0.0;
        e.last_sample = None;
        let (me, id, gen) = (self.clone(), e.id, e.run_gen);
        tauri::async_runtime::spawn(async move {
            let r = me.bt_pipeline(id, gen).await;
            me.on_finished(id, gen, r);
        });
    }

    /// BT 流程：拿种子 → 加入会话开始下载 → 等下载完 → 从会话移除（不长期做种）。
    async fn bt_pipeline(self: &Arc<Self>, id: u64, gen: u64) -> Result<PathBuf, Option<String>> {
        let engine = self.bt_engine().await.map_err(Some)?;
        let (location, saved, dir, ctx, cancel, existing) = {
            let mut g = self.inner.lock().unwrap();
            let e = g.tasks.iter_mut().find(|t| t.id == id).ok_or(None)?;
            let b = e.bt.as_mut().ok_or(None)?;
            b.engine = Some(engine.clone());
            (b.location.clone(), b.saved_torrent.clone(), e.dir.clone(), e.ctx.clone(), b.cancel.clone(), b.handle.clone())
        };

        let handle = match existing {
            // 暂停后继续：同一个种子接着下
            Some(h) => {
                engine.resume(&h).await.map_err(|e| Some(bt::friendly(&format!("{e:#}"))))?;
                h
            }
            None => {
                // 上次保存的种子文件优先，不用再去网络上找元数据
                let source = match saved.as_ref().and_then(|p| std::fs::read(p).ok()) {
                    Some(b) => bt::Source::Bytes(b.into()),
                    None => tokio::select! {
                        _ = cancel.cancelled() => return Err(None),
                        s = bt::fetch_source(&location, &ctx) => s.map_err(Some)?,
                    },
                };
                // 磁力链接要先在 DHT / Tracker 上找元数据，可能要一会儿；期间被取消也要能退出
                let added = tokio::select! {
                    _ = cancel.cancelled() => return Err(None),
                    r = engine.add(&source, &dir, false) => r,
                };
                let (h, meta) = added.map_err(|e| Some(bt::friendly(&format!("{e:#}"))))?;
                // 记下元数据和种子文件
                let torrent_file = bt::torrent_bytes(&h).and_then(|b| {
                    let p = self.data_dir.join("torrents").join(format!("{}.torrent", meta.info_hash));
                    std::fs::create_dir_all(p.parent()?).ok()?;
                    std::fs::write(&p, &b).ok()?;
                    Some(p)
                });
                let alive = {
                    let mut g = self.inner.lock().unwrap();
                    g.dirty = true;
                    match g.tasks.iter_mut().find(|t| t.id == id && t.run_gen == gen) {
                        Some(e) => {
                            if let Some(b) = e.bt.as_mut() {
                                b.handle = Some(h.clone());
                                b.meta = Some(meta);
                                b.resolving = false;
                                if torrent_file.is_some() {
                                    b.saved_torrent = torrent_file;
                                }
                            }
                            true
                        }
                        None => false,
                    }
                };
                // 找元数据期间任务已经被删掉了：连文件一起清掉
                if !alive {
                    engine.remove(&h, true).await;
                    return Err(None);
                }
                // 找元数据期间被暂停了：停在这里
                if cancel.is_cancelled() {
                    engine.pause(&h).await;
                    return Err(None);
                }
                h
            }
        };

        // 等下载完（暂停 / 取消时退出等待）
        tokio::select! {
            _ = cancel.cancelled() => return Err(None),
            r = handle.wait_until_completed() => r.map_err(|e| Some(bt::friendly(&format!("{e:#}"))))?,
        }
        let meta = {
            let g = self.inner.lock().unwrap();
            g.tasks.iter().find(|t| t.id == id).and_then(|t| t.bt.as_ref()).and_then(|b| b.meta.clone())
        };
        let path = bt::output_path(&meta.unwrap_or_else(|| bt::meta_of(&handle)), &dir);
        engine.remove(&handle, false).await;
        {
            let mut g = self.inner.lock().unwrap();
            if let Some(b) = g.tasks.iter_mut().find(|t| t.id == id).and_then(|t| t.bt.as_mut()) {
                b.last_downloaded = b.downloaded();
                b.handle = None;
                if let Some(p) = b.saved_torrent.take() {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        Ok(path)
    }

    /// 开始（或继续）一个任务。
    fn start(self: &Arc<Self>, e: &mut Entry, connections: usize) {
        if e.bt.is_some() {
            self.spawn_bt(e);
        } else if e.video.is_some() {
            self.spawn_video(e);
        } else {
            self.spawn_run(e, connections);
        }
    }

    fn spawn_run(self: &Arc<Self>, e: &mut Entry, connections: usize) {
        let Some(url) = e.url.clone() else { return };
        let opts = TaskOptions {
            connections,
            out_dir: e.dir.clone(),
            file_name: e.name.clone(),
            referer: e.ctx.referer.clone(),
            cookie: e.ctx.cookie_header(&url),
            user_agent: e.ctx.user_agent.clone(),
            ..Default::default()
        };
        // 下载器内部用 tokio::spawn，需要在运行时上下文里调用。
        let rt = tauri::async_runtime::handle();
        let _guard = rt.inner().enter();
        let task = self.downloader_for(&url, None).start(&url, opts);
        e.progress = Some(task.progress.clone());
        e.pause = Some(task.pause_handle());
        e.run_gen += 1;
        e.status = St::Downloading;
        e.error = None;
        e.speed = 0.0;
        e.last_sample = None;
        let (me, id, gen) = (self.clone(), e.id, e.run_gen);
        tauri::async_runtime::spawn(async move {
            let r = task.wait().await.map_err(|err| match err {
                DownloadError::Cancelled => None,
                other => Some(friendly(&other)),
            });
            me.on_finished(id, gen, r);
        });
    }

    fn spawn_video(self: &Arc<Self>, e: &mut Entry) {
        let Some(v) = e.video.as_mut() else { return };
        v.cancel = CancellationToken::new();
        v.parts.lock().unwrap().clear();
        v.pauses.lock().unwrap().clear();
        v.merging = false;
        e.run_gen += 1;
        e.status = St::Downloading;
        e.error = None;
        e.speed = 0.0;
        e.last_sample = None;
        let (me, id, gen) = (self.clone(), e.id, e.run_gen);
        tauri::async_runtime::spawn(async move {
            let r = me.video_pipeline(id, gen).await;
            me.on_finished(id, gen, r);
        });
    }

    /// 视频流水线：解析 → 两路并行下载 → 合并。
    async fn video_pipeline(self: &Arc<Self>, id: u64, gen: u64) -> Result<PathBuf, Option<String>> {
        let mut retried = false;
        loop {
            // 1. 解析（没有解析过，或者地址可能过期了）
            let (page_url, cancel, need_resolve, ctx) = {
                let g = self.inner.lock().unwrap();
                let e = g.tasks.iter().find(|t| t.id == id).ok_or(None)?;
                let v = e.video.as_ref().ok_or(None)?;
                let stale = v.resolved_at.is_none_or(|t| t.elapsed() > VIDEO_INFO_TTL);
                (v.page_url.clone(), v.cancel.clone(), v.info.is_none() || stale || retried, e.ctx.clone())
            };
            if need_resolve {
                self.with_video(id, |v| v.resolving = true);
                let r = tokio::select! {
                    _ = cancel.cancelled() => return Err(None),
                    r = video::resolve(&page_url, &ctx) => r,
                };
                self.with_video(id, |v| v.resolving = false);
                let info = r.map_err(Some)?;
                self.with_video(id, |v| {
                    if v.choice.as_ref().is_some_and(|k| !info.choices.iter().any(|c| &c.key == k)) {
                        v.choice = None;
                    }
                    v.info = Some(info);
                    v.resolved_at = Some(Instant::now());
                });
            }

            // 2. 下载选中的清晰度（视频、音频两路并行）
            let (choice, tmp, parts, pauses, use_cookies) = {
                let g = self.inner.lock().unwrap();
                let e = g.tasks.iter().find(|t| t.id == id).ok_or(None)?;
                let v = e.video.as_ref().ok_or(None)?;
                (
                    v.chosen().cloned().ok_or(Some("没有可以下载的清晰度".to_string()))?,
                    v.tmp_dir.clone(),
                    v.parts.clone(),
                    v.pauses.clone(),
                    v.info.as_ref().is_some_and(|i| i.use_cookies),
                )
            };
            std::fs::create_dir_all(&tmp).map_err(|e| Some(format!("无法创建临时文件夹：{e}")))?;
            parts.lock().unwrap().clear();
            pauses.lock().unwrap().clear();
            let raw_streams: Vec<video::Stream> = [choice.video.clone(), choice.audio.clone()].into_iter().flatten().collect();
            // B站等：测速挑最快的 CDN 节点（视频、音频同时测）
            let streams: Vec<video::Stream> = tokio::select! {
                _ = cancel.cancelled() => return Err(None),
                s = futures_util::future::join_all(raw_streams.into_iter().map(video::pick_fastest_mirror)) => s,
            };
            // 每一路：已经下好的直接用；否则启动下载，结束后用下载器实际返回的路径
            // （换了 CDN 节点、服务器文件标识不同时，下载器可能另起一个名字）。
            let mut futs = Vec::new();
            for s in &streams {
                let file = tmp.join(format!("{}.{}", s.format_id, s.ext));
                if file.exists() {
                    futs.push(tauri::async_runtime::spawn(async move { Ok::<PathBuf, PartErr>(file) }));
                    continue;
                }
                if s.direct {
                    let opts = TaskOptions {
                        connections: VIDEO_CONNECTIONS,
                        out_dir: tmp.clone(),
                        file_name: Some(format!("{}.{}", s.format_id, s.ext)),
                        headers: s.headers.iter().filter(|(k, _)| !k.eq_ignore_ascii_case("user-agent")).cloned().collect(),
                        user_agent: s.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("user-agent")).map(|(_, v)| v.clone()),
                        ..Default::default()
                    };
                    let task = {
                        let rt = tauri::async_runtime::handle();
                        let _guard = rt.inner().enter();
                        self.downloader_for(&s.url, Some(&page_url)).start(&s.url, opts)
                    };
                    parts.lock().unwrap().push((task.progress.clone(), s.size));
                    pauses.lock().unwrap().push(task.pause_handle());
                    // 我们的引擎下不下来（被拒绝、限流、地址过期），这一路改交给 yt-dlp 下载，而不是直接报错
                    let cx = if use_cookies { ctx.clone() } else { ReqCtx { cookies: Vec::new(), ..ctx.clone() } };
                    let (page, fid, out, c) = (page_url.clone(), s.format_id.clone(), file.clone(), cancel.clone());
                    futs.push(tauri::async_runtime::spawn(async move {
                        match task.wait().await {
                            Ok(p) => Ok(p),
                            Err(DownloadError::Cancelled) => Err(PartErr::Cancelled),
                            Err(_) if c.is_cancelled() => Err(PartErr::Cancelled),
                            Err(first) => match {
                                // 我们的 .part 是分段乱序写的，yt-dlp 会当成自己的半截文件接着写，必须先删掉
                                let _ = std::fs::remove_file(part_path(&out));
                                let _ = std::fs::remove_file(state_path(&out));
                                video::ytdlp_download(&page, &fid, &out, &c, &cx).await
                            } {
                                Ok(()) => Ok(out),
                                Err(_) if c.is_cancelled() => Err(PartErr::Cancelled),
                                Err(_) if matches!(first, DownloadError::Status(401 | 403 | 410)) => Err(PartErr::Expired),
                                Err(m) => Err(PartErr::Msg(m)),
                            },
                        }
                    }));
                } else {
                    // 和解析时一样带不带 Cookie，格式编号才对得上
                    let cx = if use_cookies { ctx.clone() } else { ReqCtx { cookies: Vec::new(), ..ctx.clone() } };
                    let (page, fid, out, c) = (page_url.clone(), s.format_id.clone(), file.clone(), cancel.clone());
                    futs.push(tauri::async_runtime::spawn(async move {
                        video::ytdlp_download(&page, &fid, &out, &c, &cx).await.map(|_| out).map_err(PartErr::Msg)
                    }));
                }
            }
            let mut expired = false;
            let mut first_err: Option<Option<String>> = None;
            let mut files = Vec::new();
            for f in futs {
                match f.await {
                    Ok(Ok(p)) => files.push(p),
                    Ok(Err(PartErr::Cancelled)) => first_err = first_err.or(Some(None)),
                    Ok(Err(PartErr::Expired)) => expired = true,
                    Ok(Err(PartErr::Msg(m))) => {
                        // 一路失败了，另一路也停下
                        for p in pauses.lock().unwrap().iter() {
                            p.pause();
                        }
                        first_err = first_err.or(Some(Some(m)));
                    }
                    Err(e) => first_err = first_err.or(Some(Some(e.to_string()))),
                }
            }
            if cancel.is_cancelled() {
                return Err(None);
            }
            if expired && !retried {
                // 视频地址过期：重新解析一次再接着下（已下的部分保留）
                retried = true;
                continue;
            }
            if let Some(e) = first_err {
                return Err(e);
            }
            if expired {
                return Err(Some("视频地址一直无法访问，可能需要登录".into()));
            }

            // 3. 合并成一个文件
            let (dir, name) = {
                let g = self.inner.lock().unwrap();
                let e = g.tasks.iter().find(|t| t.id == id && t.run_gen == gen).ok_or(None)?;
                (e.dir.clone(), e.display_name())
            };
            std::fs::create_dir_all(&dir).map_err(|e| Some(format!("无法创建保存位置：{e}")))?;
            let out = panda_http::filename::unique_path(&dir, &panda_http::filename::sanitize(&name), |p| p.exists());
            self.with_video(id, |v| v.merging = true);
            let r = match files.as_slice() {
                [single] if single.extension() == out.extension() => sys::move_file(single, &out).map_err(|e| e.to_string()),
                [single] => video::merge(single, None, &out, &cancel).await,
                [v, a] => video::merge(v, Some(a), &out, &cancel).await,
                _ => Err("没有下载到任何内容".into()),
            };
            self.with_video(id, |v| v.merging = false);
            if let Err(msg) = r {
                let _ = std::fs::remove_file(&out);
                return Err(if cancel.is_cancelled() { None } else { Some(msg) });
            }
            remove_dir_quiet(&tmp);
            return Ok(out);
        }
    }

    fn with_video(&self, id: u64, f: impl FnOnce(&mut VideoJob)) {
        let mut g = self.inner.lock().unwrap();
        if let Some(v) = g.tasks.iter_mut().find(|t| t.id == id).and_then(|t| t.video.as_mut()) {
            f(v);
        }
        g.dirty = true;
    }

    /// 一次运行结束。`Err(None)` 表示被暂停或取消。
    fn on_finished(&self, id: u64, gen: u64, r: Result<PathBuf, Option<String>>) {
        let mut g = self.inner.lock().unwrap();
        let Some(idx) = g.tasks.iter().position(|t| t.id == id) else { return };
        if g.tasks[idx].run_gen != gen {
            return;
        }
        match r {
            Ok(path) => {
                let size = sys::size_of(&path);
                let e = &g.tasks[idx];
                let url = e
            .video
            .as_ref()
            .map(|v| v.page_url.clone())
            .or_else(|| e.bt.as_ref().map(|b| b.location.clone()))
            .or_else(|| e.url.clone())
            .unwrap_or_default();
                sys::mark_from_internet(&path, &url);
                g.history.add(HistoryItem { url, path: path.clone(), size, finished_at: store::now_secs() });
                store::save(&self.data_dir.join("history.json"), &g.history);
                let e = &mut g.tasks[idx];
                e.status = St::Done;
                e.final_path = Some(path);
                e.speed = 0.0;
            }
            Err(None) => {
                let e = &mut g.tasks[idx];
                if e.status == St::Downloading && !e.relocating {
                    e.status = St::Paused;
                }
            }
            Err(Some(msg)) => {
                let e = &mut g.tasks[idx];
                e.status = St::Failed;
                e.error = Some(msg);
            }
        }
        g.dirty = true;
    }

    fn with<R>(&self, id: u64, f: impl FnOnce(&mut Entry) -> R) -> Option<R> {
        let mut g = self.inner.lock().unwrap();
        g.dirty = true;
        g.tasks.iter_mut().find(|t| t.id == id).map(f)
    }

    pub fn pause(&self, id: u64) {
        self.with(id, |e| {
            if e.status == St::Downloading {
                e.stop();
                e.status = St::Paused;
            }
        });
    }

    pub fn resume(self: &Arc<Self>, id: u64) {
        let mut g = self.inner.lock().unwrap();
        let conns = g.settings.connections;
        if let Some(e) = g.tasks.iter_mut().find(|t| t.id == id) {
            if matches!(e.status, St::Paused | St::Failed) && e.unsupported.is_none() && e.dup_of.is_none() {
                self.start(e, conns);
            }
        }
        g.dirty = true;
    }

    pub fn pause_all(&self) {
        let mut g = self.inner.lock().unwrap();
        for e in g.tasks.iter_mut().filter(|e| e.status == St::Downloading) {
            e.stop();
            e.status = St::Paused;
        }
        g.dirty = true;
    }

    /// 取消：先停下，5 秒内可以撤销，过了时间才删除数据。
    pub fn cancel(&self, id: u64) {
        self.with(id, |e| {
            if matches!(e.status, St::Downloading | St::Paused | St::Failed) {
                e.stop();
                e.status = St::Cancelled;
                e.cancel_at = Some(Instant::now() + UNDO_WINDOW);
            }
        });
    }

    pub fn undo_cancel(self: &Arc<Self>, id: u64) {
        let mut g = self.inner.lock().unwrap();
        let conns = g.settings.connections;
        if let Some(e) = g.tasks.iter_mut().find(|t| t.id == id && t.status == St::Cancelled) {
            e.cancel_at = None;
            self.start(e, conns);
        }
        g.dirty = true;
    }

    /// 确认窗里换了清晰度：停下当前的预下载，按新清晰度重新开始。
    pub fn set_video_choice(self: &Arc<Self>, id: u64, key: String) {
        let mut g = self.inner.lock().unwrap();
        if let Some(e) = g.tasks.iter_mut().find(|t| t.id == id) {
            let Some(v) = e.video.as_mut() else { return };
            if v.choice.as_deref() == Some(&key) {
                return;
            }
            v.choice = Some(key);
            e.name = None;
            if e.status == St::Downloading {
                e.stop();
                self.spawn_video(e);
            }
        }
        g.dirty = true;
    }

    /// 确认窗：开始下载。名字或目录改了就把已下的数据挪过去，接着下。
    pub async fn confirm(self: Arc<Self>, group: u64, items: Vec<ConfirmItem>) {
        let chosen: Vec<u64> = items.iter().map(|i| i.id).collect();
        // 批量里没勾选的直接丢弃。
        let dropped: Vec<u64> = {
            let g = self.inner.lock().unwrap();
            g.tasks.iter().filter(|t| t.group == group && !t.confirmed && !chosen.contains(&t.id)).map(|t| t.id).collect()
        };
        for id in dropped {
            self.discard(id).await;
        }
        for item in items {
            self.confirm_one(item).await;
        }
    }

    async fn confirm_one(self: &Arc<Self>, item: ConfirmItem) {
        let name = panda_http::filename::sanitize(item.name.trim());
        let dir = PathBuf::from(item.dir.trim());
        // BT：名字由种子决定；换了保存位置就从会话里移除（连已下的部分），在新位置重新开始
        let bt_move = {
            let mut g = self.inner.lock().unwrap();
            g.dirty = true;
            match g.tasks.iter_mut().find(|t| t.id == item.id) {
                Some(e) if e.bt.is_some() => {
                    e.confirmed = true;
                    if e.dir == dir || e.status == St::Done {
                        return;
                    }
                    let b = e.bt.as_mut().unwrap();
                    b.cancel.cancel();
                    e.run_gen += 1;
                    Some((b.engine.clone(), b.handle.take()))
                }
                _ => None,
            }
        };
        if let Some((engine, handle)) = bt_move {
            if let (Some(en), Some(h)) = (engine, handle) {
                en.remove(&h, true).await;
            }
            let mut g = self.inner.lock().unwrap();
            if let Some(e) = g.tasks.iter_mut().find(|t| t.id == item.id) {
                e.dir = dir;
                self.spawn_bt(e);
            }
            return;
        }
        let progress = {
            let mut g = self.inner.lock().unwrap();
            let conns = g.settings.connections;
            // 记住这类文件这次存在哪，下次默认还放这里。
            let cat = category::category_of(&name);
            g.settings.category_dirs.insert(cat.key().to_string(), dir.clone());
            store::save(&self.data_dir.join("settings.json"), &g.settings);
            g.dirty = true;
            let Some(e) = g.tasks.iter_mut().find(|t| t.id == item.id) else { return };
            e.confirmed = true;
            if e.unsupported.is_some() {
                e.status = St::Cancelled;
                e.cancel_at = Some(Instant::now());
                return;
            }
            if e.video.is_some() {
                // 视频最后才合并输出，改名换目录直接生效，不用挪文件。
                if e.status == St::Done {
                    if let Some(old) = e.final_path.clone() {
                        if old != dir.join(&name) {
                            let dest = panda_http::filename::unique_path(&dir, &name, |p| p.exists() && p != old);
                            if sys::move_file(&old, &dest).is_ok() {
                                e.final_path = Some(dest);
                            }
                        }
                    }
                } else {
                    e.name = Some(name);
                    e.dir = dir;
                }
                return;
            }
            if e.dup_of.take().is_some() {
                // "再下一份"：下载器会自动改名为 "名字 (1)"。
                e.name = Some(name);
                e.dir = dir;
                self.spawn_run(e, conns);
                return;
            }
            let same = e.current_path().is_some_and(|p| p == dir.join(&name));
            if same {
                return;
            }
            e.relocating = true;
            e.stop();
            e.progress.clone()
        };
        // 等当前这次下载停下来（进度已保存）。
        if let Some(p) = &progress {
            for _ in 0..200 {
                if matches!(p.status(), Status::Paused | Status::Failed | Status::Done) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        let mut g = self.inner.lock().unwrap();
        let conns = g.settings.connections;
        let Some(e) = g.tasks.iter_mut().find(|t| t.id == item.id) else { return };
        e.relocating = false;
        let target = dir.join(&name);
        if e.status == St::Done {
            if let Some(old) = e.final_path.clone() {
                let dest = panda_http::filename::unique_path(&dir, &name, |p| p.exists() && p != old);
                if sys::move_file(&old, &dest).is_ok() {
                    e.final_path = Some(dest);
                }
            }
            return;
        }
        if let Some(old) = progress.as_ref().and_then(|p| p.path()) {
            let _ = sys::move_file(&part_path(&old), &part_path(&target));
            let _ = sys::move_file(&state_path(&old), &state_path(&target));
        }
        e.name = Some(name);
        e.dir = dir;
        self.spawn_run(e, conns);
    }

    /// 确认窗点了取消：停下并删除预下载的数据。
    pub async fn cancel_group(self: Arc<Self>, group: u64) {
        let ids: Vec<u64> = {
            let g = self.inner.lock().unwrap();
            g.tasks.iter().filter(|t| t.group == group && !t.confirmed).map(|t| t.id).collect()
        };
        for id in ids {
            self.discard(id).await;
        }
    }

    async fn discard(&self, id: u64) {
        let progress = self.with(id, |e| {
            e.stop();
            e.run_gen += 1;
            e.progress.clone()
        });
        if let Some(Some(p)) = &progress {
            for _ in 0..100 {
                if !matches!(p.status(), Status::Downloading | Status::Probing) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        } else {
            // 视频任务：给两路下载一点时间释放文件
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        let removed = {
            let mut g = self.inner.lock().unwrap();
            g.dirty = true;
            g.tasks.iter().position(|t| t.id == id).map(|idx| g.tasks.remove(idx))
        };
        if let Some(e) = removed {
            delete_partial(&e);
        }
    }

    /// 重复下载提示里点了"打开已有的"。
    pub fn open_existing(&self, group: u64) {
        let mut g = self.inner.lock().unwrap();
        let mut opened = false;
        g.tasks.retain(|t| {
            if t.group == group && !t.confirmed {
                if let (Some(p), false) = (&t.dup_of, opened) {
                    let _ = sys::reveal(p);
                    opened = true;
                }
                return t.dup_of.is_none();
            }
            true
        });
        g.dirty = true;
    }

    pub fn dismiss_done(&self, ids: Vec<u64>) {
        let mut g = self.inner.lock().unwrap();
        for e in g.tasks.iter_mut().filter(|e| ids.contains(&e.id)) {
            e.done_dismissed = true;
        }
        g.dirty = true;
    }

    /// 从列表里移除（已完成的只移除记录，不删文件）。
    pub fn remove(&self, id: u64) {
        let mut g = self.inner.lock().unwrap();
        if let Some(idx) = g.tasks.iter().position(|t| t.id == id) {
            let e = g.tasks.remove(idx);
            if e.status != St::Done {
                e.stop();
            }
        }
        g.dirty = true;
    }

    fn path_of(&self, id: u64) -> Option<PathBuf> {
        let g = self.inner.lock().unwrap();
        g.tasks.iter().find(|t| t.id == id).and_then(|t| t.final_path.clone().or_else(|| t.dup_of.clone()))
    }

    pub fn open_file(&self, id: u64) -> Result<(), String> {
        sys::shell_open(&self.path_of(id).ok_or("文件还没下完")?)
    }

    pub fn open_folder(&self, id: u64) -> Result<(), String> {
        let g = self.inner.lock().unwrap();
        let e = g.tasks.iter().find(|t| t.id == id).ok_or("任务不存在")?;
        match e.current_path().or_else(|| e.dup_of.clone()) {
            Some(p) => sys::reveal(&p),
            None => sys::shell_open(&e.dir),
        }
    }

    /// 完成窗的主按钮：按文件类型做最可能的下一步。返回给用户看的提示（可选）。
    pub async fn primary_action(&self, id: u64) -> Result<Option<String>, String> {
        let path = self.path_of(id).ok_or("文件还没下完")?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match category::action_of(&name) {
            Action::Extract => {
                let p = path.clone();
                let dest = tauri::async_runtime::spawn_blocking(move || sys::extract_zip(&p))
                    .await
                    .map_err(|e| e.to_string())??;
                sys::shell_open(&dest)?;
                Ok(Some(format!("已解压到 {}", dest.display())))
            }
            Action::Install => {
                let p = path.clone();
                let r = tauri::async_runtime::spawn_blocking(move || sys::defender_scan(&p)).await.map_err(|e| e.to_string())?;
                match r {
                    sys::ScanResult::Threat => Err("Windows Defender 发现威胁，已阻止运行".into()),
                    sys::ScanResult::Clean => sys::shell_open(&path).map(|_| Some("扫描通过".into())),
                    sys::ScanResult::Unknown => sys::shell_open(&path).map(|_| None),
                }
            }
            _ => sys::shell_open(&path).map(|_| None),
        }
    }

    /// 从"已完成"里移除一条记录（不删文件）。
    pub fn remove_history(&self, path: &str) {
        let mut g = self.inner.lock().unwrap();
        g.history.items.retain(|h| h.path.to_string_lossy() != path);
        store::save(&self.data_dir.join("history.json"), &g.history);
    }

    pub fn history(&self) -> Vec<HistoryItem> {
        let g = self.inner.lock().unwrap();
        g.history.items.iter().rev().take(300).cloned().collect()
    }

    pub fn group_views(&self, group: u64) -> Vec<TaskView> {
        let g = self.inner.lock().unwrap();
        g.tasks.iter().filter(|t| t.group == group).map(|t| view(t, true)).collect()
    }

    pub fn views(&self) -> Vec<TaskView> {
        let g = self.inner.lock().unwrap();
        g.tasks.iter().map(|t| view(t, !t.confirmed)).collect()
    }

    /// 每 500 毫秒：算速度、处理到期的取消、控制各窗口的显示、推送进度。
    pub fn tick(self: &Arc<Self>, n: u64) {
        let busy = sys::user_is_busy();
        let now = Instant::now();
        let (views, show_float, show_done, confirms, float_pos, save) = {
            let mut g = self.inner.lock().unwrap();
            for e in g.tasks.iter_mut() {
                let d = e.downloaded();
                if let Some((t0, d0)) = e.last_sample {
                    let dt = now.duration_since(t0).as_secs_f64().max(0.001);
                    let inst = d.saturating_sub(d0) as f64 / dt;
                    e.speed = if e.status == St::Downloading { e.speed * 0.6 + inst * 0.4 } else { 0.0 };
                }
                e.last_sample = Some((now, d));
            }
            // 撤销时间已过：真正删除。
            let expired: Vec<usize> =
                g.tasks.iter().enumerate().filter(|(_, t)| t.cancel_at.is_some_and(|at| now >= at)).map(|(i, _)| i).collect();
            let mut doomed = Vec::new();
            for i in expired.into_iter().rev() {
                doomed.push(g.tasks.remove(i));
                g.dirty = true;
            }
            let show_float = !busy && g.tasks.iter().any(Entry::is_active);
            let show_done = !busy && g.tasks.iter().any(|t| t.confirmed && t.status == St::Done && !t.done_dismissed);
            let confirms = if busy { Vec::new() } else { std::mem::take(&mut g.pending_confirm) };
            let save = std::mem::take(&mut g.dirty) || n % 20 == 0;
            let views: Vec<TaskView> = g.tasks.iter().map(|t| view(t, !t.confirmed)).collect();
            if !doomed.is_empty() {
                std::thread::spawn(move || doomed.iter().for_each(delete_partial));
            }
            (views, show_float, show_done, confirms, g.settings.float_pos, save)
        };
        if save {
            self.persist();
        }
        windows::set_float_visible(&self.app, show_float, float_pos);
        windows::set_done_visible(&self.app, show_done);
        for gid in confirms {
            windows::open_confirm(&self.app, gid);
        }
        let _ = self.app.emit("tasks", &views);
    }

    /// 保存未完成的任务，重启后继续。
    pub fn persist(&self) {
        let g = self.inner.lock().unwrap();
        let saved: Vec<SavedTask> = g
            .tasks
            .iter()
            .filter(|t| t.confirmed && t.url.is_some() && matches!(t.status, St::Downloading | St::Paused | St::Failed))
            .map(|t| SavedTask {
                link: t.link.clone(),
                url: t.url.clone().unwrap(),
                dir: t.dir.clone(),
                name: if t.video.is_some() {
                    t.name.clone()
                } else {
                    t.progress
                        .as_ref()
                        .and_then(|p| p.path())
                        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                        .or_else(|| t.name.clone())
                },
                paused: t.status != St::Downloading,
                video: t.video.as_ref().map(|v| SavedVideo {
                    page_url: v.page_url.clone(),
                    choice: v.choice.clone().or_else(|| v.chosen().map(|c| c.key.clone())),
                    tmp_dir: v.tmp_dir.clone(),
                }),
                bt: t.bt.as_ref().map(|b| SavedBt { location: b.location.clone(), torrent_file: b.saved_torrent.clone(), meta: b.meta.clone(), downloaded: b.downloaded() }),
            })
            .collect();
        store::save(&self.data_dir.join("tasks.json"), &saved);
    }
}

fn delete_partial(e: &Entry) {
    if e.status == St::Done {
        return;
    }
    if let Some(b) = &e.bt {
        b.discard(&e.dir);
        return;
    }
    if let Some(v) = &e.video {
        remove_dir_quiet(&v.tmp_dir);
        return;
    }
    if let Some(p) = e.progress.as_ref().and_then(|p| p.path()) {
        remove_quiet(&part_path(&p));
        remove_quiet(&state_path(&p));
    }
}

fn remove_quiet(p: &Path) {
    // 下载线程可能还没完全释放文件，稍后重试几次。
    for _ in 0..10 {
        if !p.exists() || std::fs::remove_file(p).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn remove_dir_quiet(p: &Path) {
    for _ in 0..10 {
        if !p.exists() || std::fs::remove_dir_all(p).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    // 临时总文件夹空了就一起删掉
    if let Some(parent) = p.parent() {
        let _ = std::fs::remove_dir(parent);
    }
}

fn view(t: &Entry, with_space: bool) -> TaskView {
    let name = t.display_name();
    let mut cat = category::category_of(&name);
    let mut action = category::action_of(&name);
    let (mut total, downloaded, mut resumable, mut probing) = match &t.progress {
        Some(p) => (p.total(), p.downloaded(), p.resumable(), p.status() == Status::Probing),
        None => (None, 0, false, false),
    };
    let mut downloaded = downloaded;
    let video = t.video.as_ref().map(|v| {
        let (d, tot) = v.progress();
        downloaded = d;
        total = tot;
        resumable = true;
        probing = v.resolving;
        if v.info.is_none() {
            cat = Category::Video;
            action = Action::Play;
        }
        VideoView {
            resolving: v.resolving,
            merging: v.merging,
            title: v.info.as_ref().map(|i| i.title.clone()),
            thumbnail: v.info.as_ref().and_then(|i| i.thumbnail.clone()),
            duration: v.info.as_ref().and_then(|i| i.duration),
            uploader: v.info.as_ref().and_then(|i| i.uploader.clone()),
            site: v.info.as_ref().map(|i| i.site.clone()),
            choice: v.chosen().map(|c| c.key.clone()),
            choices: v
                .info
                .as_ref()
                .map(|i| {
                    i.choices
                        .iter()
                        .map(|c| ChoiceView { key: c.key.clone(), label: c.label.clone(), detail: c.detail.clone(), size: c.size })
                        .collect()
                })
                .unwrap_or_default(),
        }
    });
    let bt = t.bt.as_ref().map(|b| {
        let live = b.live();
        resumable = true;
        probing = b.resolving;
        downloaded = b.downloaded();
        if let Some(l) = &live {
            if l.total > 0 {
                total = Some(l.total);
            }
        }
        let meta = b.meta.as_ref();
        if let Some(m) = meta {
            if total.is_none() && m.total > 0 {
                total = Some(m.total);
            }
        }
        // 多文件种子是文件夹：图标归"其他"，主按钮"打开"（打开文件夹）
        if meta.map(|m| m.files != 1).unwrap_or(true) {
            cat = Category::Other;
            action = Action::Open;
        }
        BtView {
            resolving: b.resolving,
            peers: live.as_ref().map(|l| l.peers).unwrap_or(0),
            files: meta.map(|m| m.files).unwrap_or(0),
            private: meta.map(|m| m.private).unwrap_or(false),
            upload_speed: live.as_ref().map(|l| l.upload_speed).unwrap_or(0),
        }
    });
    TaskView {
        id: t.id,
        group: t.group,
        category: cat.key(),
        action,
        action_label: action.label(),
        name,
        dir: t.dir.display().to_string(),
        path: t.current_path().map(|p| p.display().to_string()),
        url: t.url.clone().unwrap_or_else(|| t.link.clone()),
        total,
        downloaded,
        speed: t.speed as u64,
        status: t.status,
        confirmed: t.confirmed,
        error: t.error.clone(),
        resumable,
        probing,
        dup_of: t.dup_of.as_ref().map(|p| p.display().to_string()),
        unsupported: t.unsupported.clone(),
        undo_ms: t.cancel_at.map(|at| at.saturating_duration_since(Instant::now()).as_millis() as u64),
        free_space: if with_space { sys::free_space(&t.dir) } else { None },
        dismissed: t.done_dismissed,
        video,
        bt,
    }
}
