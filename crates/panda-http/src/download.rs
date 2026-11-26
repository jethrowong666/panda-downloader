use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use reqwest::header::{self, HeaderMap};
use reqwest::{RequestBuilder, Response, StatusCode};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::segments::{Job, Scheduler, MAX_CHUNK};
use crate::state::{part_path, state_path, ResumeState};
use crate::{filename, ClientConfig, DownloadError, Progress, RateLimiter, Status, TaskOptions};

type Result<T> = std::result::Result<T, DownloadError>;

/// CDN 临时地址过期后，最多回原始地址刷新几次。
const MAX_URL_REFRESH: u32 = 3;

/// 下载器：持有连接池和全局限速器，可以同时跑很多任务。
#[derive(Clone)]
pub struct Downloader {
    client: reqwest::Client,
    limiter: Arc<RateLimiter>,
}

/// 一个正在运行的下载任务。
pub struct Task {
    pub progress: Arc<Progress>,
    cancel: CancellationToken,
    handle: tokio::task::JoinHandle<Result<PathBuf>>,
}

/// 可以单独保存的暂停开关（`Task::wait` 会拿走 Task，界面线程用它来暂停）。
#[derive(Clone)]
pub struct PauseHandle(CancellationToken);

impl PauseHandle {
    pub fn pause(&self) {
        self.0.cancel();
    }
}

impl Task {
    /// 暂停：保存进度后停止。之后用同样的网址和目录重新 `start` 即可续传。
    pub fn pause(&self) {
        self.cancel.cancel();
    }

    pub fn pause_handle(&self) -> PauseHandle {
        PauseHandle(self.cancel.clone())
    }

    /// 等待任务结束，成功时返回最终文件路径。
    pub async fn wait(self) -> Result<PathBuf> {
        match self.handle.await {
            Ok(r) => r,
            Err(e) => Err(DownloadError::Io(std::io::Error::other(e.to_string()))),
        }
    }

    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }
}

impl Downloader {
    pub fn new(cfg: &ClientConfig) -> Result<Self> {
        Self::with_limiter(cfg, Arc::new(RateLimiter::unlimited()))
    }

    /// 传入共享的限速器，多个下载器共用就是全局限速。
    pub fn with_limiter(cfg: &ClientConfig, limiter: Arc<RateLimiter>) -> Result<Self> {
        let mut b = reqwest::Client::builder()
            .user_agent(cfg.user_agent.clone())
            .connect_timeout(cfg.connect_timeout)
            .redirect(reqwest::redirect::Policy::limited(10))
            .pool_max_idle_per_host(32)
            .tcp_nodelay(true)
            .http2_adaptive_window(true);
        if cfg.direct {
            b = b.no_proxy();
        } else if let Some(p) = &cfg.proxy {
            b = b.proxy(reqwest::Proxy::all(p).map_err(|e| DownloadError::InvalidUrl(format!("代理地址无效: {e}")))?);
        }
        Ok(Self { client: b.build()?, limiter })
    }

    pub fn limiter(&self) -> &Arc<RateLimiter> {
        &self.limiter
    }

    /// 开始下载。如果目标目录里有同一文件的断点记录，会自动续传。
    pub fn start(&self, url: &str, opts: TaskOptions) -> Task {
        let progress = Arc::new(Progress::default());
        let cancel = CancellationToken::new();
        let ctx = TaskCtx {
            client: self.client.clone(),
            limiter: self.limiter.clone(),
            opts,
            progress: progress.clone(),
            cancel: cancel.clone(),
        };
        let url = url.to_string();
        let handle = tokio::spawn(async move {
            let r = ctx.run(&url).await;
            ctx.progress.set_status(match &r {
                Ok(_) => Status::Done,
                Err(DownloadError::Cancelled) => Status::Paused,
                Err(_) => Status::Failed,
            });
            r
        });
        Task { progress, cancel, handle }
    }
}

struct TaskCtx {
    client: reqwest::Client,
    limiter: Arc<RateLimiter>,
    opts: TaskOptions,
    progress: Arc<Progress>,
    cancel: CancellationToken,
}

/// 探测结果。
struct RemoteInfo {
    final_url: Url,
    total: Option<u64>,
    ranges: bool,
    etag: Option<String>,
    last_modified: Option<String>,
    name: Option<String>,
    /// 服务器不支持分段时，探测请求本身就是完整下载，直接复用，免得再请求一次。
    body: Option<Response>,
}

impl TaskCtx {
    fn request(&self, url: Url) -> RequestBuilder {
        let mut r = self
            .client
            .get(url)
            // 必须拿原始字节，否则分段偏移会错乱。
            .header(header::ACCEPT_ENCODING, "identity");
        if let Some(ua) = &self.opts.user_agent {
            r = r.header(header::USER_AGENT, ua);
        }
        if let Some(v) = &self.opts.referer {
            r = r.header(header::REFERER, v);
        }
        if let Some(v) = &self.opts.cookie {
            r = r.header(header::COOKIE, v);
        }
        for (k, v) in &self.opts.headers {
            r = r.header(k.as_str(), v.as_str());
        }
        r
    }

    async fn send(&self, req: RequestBuilder) -> Result<Response> {
        tokio::select! {
            _ = self.cancel.cancelled() => Err(DownloadError::Cancelled),
            r = req.send() => Ok(r?),
        }
    }

    async fn sleep(&self, d: Duration) -> Result<()> {
        tokio::select! {
            _ = self.cancel.cancelled() => Err(DownloadError::Cancelled),
            _ = tokio::time::sleep(d) => Ok(()),
        }
    }

    async fn probe(&self, url: &Url) -> Result<RemoteInfo> {
        let resp = self.send(self.request(url.clone()).header(header::RANGE, "bytes=0-0")).await?;
        let status = resp.status();
        let h = resp.headers().clone();
        let final_url = resp.url().clone();
        let etag = header_str(&h, header::ETAG);
        let last_modified = header_str(&h, header::LAST_MODIFIED);
        let name = h
            .get(header::CONTENT_DISPOSITION)
            .and_then(|v| filename::from_content_disposition(v.as_bytes()))
            .or_else(|| filename::from_url(&final_url));
        let content_range_total = header_str(&h, header::CONTENT_RANGE).and_then(|v| parse_content_range(&v).map(|r| r.2));

        let mut info = RemoteInfo { final_url, total: None, ranges: false, etag, last_modified, name, body: None };
        match status {
            StatusCode::PARTIAL_CONTENT => {
                // 返回了 206 就说明支持 Range；大小可能是 "*"（未知），那就走顺序续传。
                info.total = content_range_total.flatten();
                info.ranges = true;
            }
            StatusCode::RANGE_NOT_SATISFIABLE => {
                // 空文件对 bytes=0-0 会返回 416，Content-Range: bytes */0。
                if content_range_total.flatten() == Some(0) {
                    info.total = Some(0);
                } else {
                    let resp = self.send(self.request(url.clone())).await?;
                    if !resp.status().is_success() {
                        return Err(DownloadError::Status(resp.status().as_u16()));
                    }
                    info.total = resp.content_length();
                    info.body = Some(resp);
                }
            }
            s if s.is_success() => {
                info.total = resp.content_length();
                info.body = Some(resp);
            }
            s => return Err(DownloadError::Status(s.as_u16())),
        }
        Ok(info)
    }

    async fn probe_with_retry(&self, url: &Url) -> Result<RemoteInfo> {
        let mut attempt = 0;
        loop {
            match self.probe(url).await {
                Ok(i) => return Ok(i),
                Err(e) if e.is_fatal() => return Err(e),
                Err(e) => {
                    attempt += 1;
                    if attempt > self.opts.max_retries {
                        return Err(DownloadError::RetriesExhausted { attempts: attempt - 1, last: e.to_string() });
                    }
                    self.sleep(backoff(attempt)).await?;
                }
            }
        }
    }

    async fn run(&self, url_str: &str) -> Result<PathBuf> {
        let url = Url::parse(url_str.trim()).map_err(|e| DownloadError::InvalidUrl(format!("{url_str}: {e}")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(DownloadError::InvalidUrl(format!("HTTP 引擎不处理 {} 协议", url.scheme())));
        }
        self.progress.set_status(Status::Probing);
        let mut info = self.probe_with_retry(&url).await?;
        self.progress.set_total(info.total);
        let segmented = info.ranges && info.total.is_some_and(|t| t > 0);
        // 只要服务器支持 Range 就能续传；大小未知时走顺序续传。
        self.progress.set_resumable(info.ranges);

        let name = filename::sanitize(
            self.opts.file_name.as_deref().or(info.name.as_deref()).unwrap_or("download"),
        );
        std::fs::create_dir_all(&self.opts.out_dir)?;
        let dir = &self.opts.out_dir;

        // 看看有没有这个文件的断点记录。
        let preferred = dir.join(&name);
        let saved = ResumeState::load(&state_path(&preferred)).filter(|_| part_path(&preferred).exists());
        let discard = |p: &Path| {
            let _ = std::fs::remove_file(part_path(p));
            let _ = std::fs::remove_file(state_path(p));
        };
        let plan = match saved {
            Some(s) if s.matches(info.total, info.etag.as_deref(), info.last_modified.as_deref()) => {
                match (s.stream, segmented) {
                    (false, true) => Plan::Segments(s.segments),
                    // 上次是单连接下的，这次服务器支持分段了：已下载的前半段保留，剩下的分段下。
                    (true, true) => Plan::Convert(s.stream_pos()),
                    (true, false) => Plan::Stream(s.stream_pos()),
                    // 上次分段下载（中间有空洞），这次服务器不支持分段了，只能重来。
                    (false, false) => {
                        discard(&preferred);
                        Plan::Fresh
                    }
                }
            }
            // 同一个地址但服务器文件变了：旧数据作废，沿用原文件名重新下。
            Some(s) if s.url == url_str => {
                discard(&preferred);
                Plan::Fresh
            }
            // 不同地址恰好同名，是另一个文件的断点，别动它，换个名字。
            _ => Plan::Fresh,
        };
        let final_path = if matches!(plan, Plan::Fresh) {
            filename::unique_path(dir, &name, |p| p.exists() || part_path(p).exists())
        } else {
            preferred
        };
        self.progress.set_path(final_path.clone());
        let part = part_path(&final_path);
        let state_file = state_path(&final_path);

        let file = OpenOptions::new().create(true).write(true).truncate(false).open(&part)?;
        match &plan {
            Plan::Fresh => {
                file.set_len(0)?;
                if segmented {
                    // 预先分配空间：减少磁盘碎片，空间不够时立刻报错而不是下到一半才失败。
                    file.set_len(info.total.unwrap())?;
                }
            }
            Plan::Convert(_) => file.set_len(info.total.unwrap())?,
            // 截掉记录之后可能存在的未确认数据。
            Plan::Stream(pos) => file.set_len(*pos)?,
            Plan::Segments(_) => {}
        }
        let file = Arc::new(file);

        self.progress.set_status(Status::Downloading);
        let body = info.body.take();
        let result = if info.total == Some(0) {
            Ok(())
        } else if segmented {
            let total = info.total.unwrap();
            let parts = self.opts.connections.clamp(1, 64);
            let parts = parts.min(total.div_ceil(self.opts.min_split.max(1)) as usize).max(1);
            let sched = match plan {
                Plan::Segments(s) => Scheduler::from_segments(s, self.opts.min_split),
                Plan::Convert(pos) => Scheduler::new_range(pos.min(total), total, parts, self.opts.min_split),
                _ => Scheduler::new(total, parts, self.opts.min_split),
            };
            self.progress.set_downloaded(total - sched.remaining());
            let seg = Arc::new(SegmentedRun {
                url: RwLock::new(info.final_url.clone()),
                original: url.clone(),
                state_url: url_str.to_string(),
                refreshes: AtomicU32::new(0),
                refresh_lock: tokio::sync::Mutex::new(()),
                sched,
                file: file.clone(),
                total,
                etag: info.etag.clone(),
                last_modified: info.last_modified.clone(),
            });
            let r = self.run_segmented(seg.clone(), &state_file).await;
            if r.is_err() {
                // 失败或暂停都保存进度，下次接着下。
                let _ = seg.state().save_durable(&file, &state_file);
            }
            r
        } else {
            let start = match plan {
                Plan::Stream(p) => p,
                _ => 0,
            };
            let st = StreamRun {
                url: url.clone(),
                state_url: url_str.to_string(),
                total: info.total,
                validators: Mutex::new((info.etag.clone(), info.last_modified.clone())),
                pos: AtomicU64::new(start),
                file: file.clone(),
            };
            self.run_stream(Arc::new(st), body, &state_file).await
        };
        result?;

        self.progress.set_status(Status::Finishing);
        file.sync_all()?;
        drop(file);
        let dest = if final_path.exists() {
            filename::unique_path(dir, &name, |p| p.exists() || part_path(p).exists())
        } else {
            final_path
        };
        std::fs::rename(&part, &dest)?;
        let _ = std::fs::remove_file(&state_file);
        self.progress.set_path(dest.clone());
        Ok(dest)
    }

    /// 顺序模式：大小未知或服务器不支持分段时使用，单连接从前往后写。
    ///
    /// 断线后用 `Range: bytes=已下载-` 接着下；服务器不认 Range（返回 200）时只能从头再来。
    async fn run_stream(&self, st: Arc<StreamRun>, mut body: Option<Response>, state_file: &Path) -> Result<()> {
        let start = st.pos.load(Ordering::SeqCst);
        self.progress.set_downloaded(start);
        if start > 0 {
            // 探测请求拿到的是从头开始的完整内容，续传用不上。
            body = None;
        }
        let saver = spawn_saver(st.file.clone(), state_file.to_path_buf(), {
            let st = st.clone();
            move || st.state()
        });
        let r = self.stream_loop(&st, body).await;
        saver.abort();
        let _ = saver.await;
        if r.is_err() {
            let _ = st.state().save_durable(&st.file, state_file);
        }
        r
    }

    async fn stream_loop(&self, st: &StreamRun, mut body: Option<Response>) -> Result<()> {
        let mut attempts = 0u32;
        loop {
            let cur = st.pos.load(Ordering::SeqCst);
            if st.total.is_some_and(|t| cur >= t) {
                return Ok(());
            }
            let r = async {
                let resp = match body.take() {
                    Some(r) => r,
                    None => {
                        let mut req = self.request(st.url.clone());
                        if cur > 0 {
                            req = req.header(header::RANGE, format!("bytes={cur}-"));
                        }
                        self.send(req).await?
                    }
                };
                let status = resp.status();
                let h = resp.headers();
                let new_etag = header_str(h, header::ETAG);
                let new_lm = header_str(h, header::LAST_MODIFIED);
                match status {
                    StatusCode::PARTIAL_CONTENT => {
                        let start = header_str(h, header::CONTENT_RANGE).and_then(|v| parse_content_range(&v)).map(|r| r.0);
                        if start != Some(cur) {
                            return Err(DownloadError::Network("服务器返回的续传位置不对".into()));
                        }
                        let old = st.validators.lock().unwrap().0.clone();
                        if let (Some(a), Some(b)) = (strong_etag(old.as_deref()), strong_etag(new_etag.as_deref())) {
                            if a != b {
                                // 文件变了：从头重下。
                                self.restart_stream(st, new_etag, new_lm)?;
                                return Ok(());
                            }
                        }
                    }
                    StatusCode::OK => {
                        if cur > 0 {
                            // 这次服务器不认续传，只能从头再来。
                            self.restart_stream(st, new_etag, new_lm)?;
                        } else {
                            *st.validators.lock().unwrap() = (new_etag, new_lm);
                        }
                    }
                    s if s == StatusCode::TOO_MANY_REQUESTS || s.is_server_error() => {
                        return Err(DownloadError::Network(format!("服务器繁忙 (HTTP {})", s.as_u16())))
                    }
                    s => return Err(DownloadError::Status(s.as_u16())),
                }

                self.progress.conn_delta(true);
                let r = async {
                    let mut stream = resp.bytes_stream();
                    while let Some(chunk) = self.next_chunk(&mut stream).await? {
                        for piece in split_bytes(chunk) {
                            self.limiter.acquire(piece.len()).await;
                            let n = piece.len() as u64;
                            let at = st.pos.load(Ordering::SeqCst);
                            write_at(st.file.clone(), at, piece).await?;
                            st.pos.store(at + n, Ordering::SeqCst);
                            self.progress.add(n);
                        }
                    }
                    Ok(())
                }
                .await;
                self.progress.conn_delta(false);
                r
            }
            .await;

            let now = st.pos.load(Ordering::SeqCst);
            let err = match r {
                Ok(()) => match st.total {
                    // 大小未知：连接正常结束就是下完了。
                    None => return Ok(()),
                    Some(t) if now >= t => return Ok(()),
                    Some(t) => DownloadError::Network(format!("连接提前断开：收到 {now} / {t} 字节")),
                },
                Err(e) if e.is_fatal() => return Err(e),
                Err(e) => e,
            };
            attempts = if now > cur { 1 } else { attempts + 1 };
            if attempts > self.opts.max_retries {
                return Err(DownloadError::RetriesExhausted { attempts: attempts - 1, last: err.to_string() });
            }
            self.sleep(backoff(attempts)).await?;
        }
    }

    fn restart_stream(&self, st: &StreamRun, etag: Option<String>, lm: Option<String>) -> Result<()> {
        st.file.set_len(0)?;
        st.pos.store(0, Ordering::SeqCst);
        *st.validators.lock().unwrap() = (etag, lm);
        self.progress.set_downloaded(0);
        Ok(())
    }

    async fn next_chunk(
        &self,
        stream: &mut (impl futures_util::Stream<Item = reqwest::Result<Bytes>> + Unpin),
    ) -> Result<Option<Bytes>> {
        let timeout = self.opts.read_timeout;
        tokio::select! {
            _ = self.cancel.cancelled() => Err(DownloadError::Cancelled),
            r = tokio::time::timeout(timeout, stream.next()) => match r {
                Err(_) => Err(DownloadError::Timeout(timeout.as_secs())),
                Ok(None) => Ok(None),
                Ok(Some(Err(e))) => Err(e.into()),
                Ok(Some(Ok(b))) => Ok(Some(b)),
            }
        }
    }

    async fn run_segmented(&self, seg: Arc<SegmentedRun>, state_file: &Path) -> Result<()> {
        let saver = spawn_saver(seg.file.clone(), state_file.to_path_buf(), {
            let seg = seg.clone();
            move || seg.state()
        });

        let workers = self.opts.connections.clamp(1, 64);
        let mut set = JoinSet::new();
        for _ in 0..workers {
            let ctx = self.clone_for_worker();
            let seg = seg.clone();
            set.spawn(async move { ctx.worker(seg).await });
        }
        let mut first_err: Option<DownloadError> = None;
        while let Some(r) = set.join_next().await {
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    let cancelled = matches!(e, DownloadError::Cancelled);
                    if first_err.is_none() || (!cancelled && matches!(first_err, Some(DownloadError::Cancelled))) {
                        first_err = Some(e);
                    }
                    // 致命错误：通知其他连接一起停下。
                    if !cancelled {
                        self.cancel.cancel();
                    }
                }
                Err(e) => first_err = first_err.or(Some(DownloadError::Io(std::io::Error::other(e.to_string())))),
            }
        }
        // 等保存任务真正停下，避免它在文件改名之后又写出一份过期的记录。
        saver.abort();
        let _ = saver.await;

        if seg.sched.is_done() {
            return Ok(());
        }
        Err(first_err.unwrap_or(DownloadError::Cancelled))
    }

    fn clone_for_worker(&self) -> TaskCtx {
        TaskCtx {
            client: self.client.clone(),
            limiter: self.limiter.clone(),
            opts: self.opts.clone(),
            progress: self.progress.clone(),
            cancel: self.cancel.clone(),
        }
    }

    async fn worker(&self, seg: Arc<SegmentedRun>) -> Result<()> {
        loop {
            if self.cancel.is_cancelled() {
                return Err(DownloadError::Cancelled);
            }
            let Some(job) = seg.sched.next_job() else { return Ok(()) };
            self.progress.conn_delta(true);
            let r = self.run_job(&seg, job).await;
            self.progress.conn_delta(false);
            seg.sched.release(job.idx);
            r?;
        }
    }

    async fn run_job(&self, seg: &SegmentedRun, job: Job) -> Result<()> {
        let mut attempts = 0u32;
        loop {
            let pos = seg.sched.pos_of(job.idx);
            let end = seg.sched.end_of(job.idx);
            if pos >= end {
                return Ok(());
            }
            let r = self.fetch_range(seg, job.idx, pos, end).await;
            let progressed = seg.sched.pos_of(job.idx) > pos;
            match r {
                Ok(()) => {
                    if seg.sched.pos_of(job.idx) >= seg.sched.end_of(job.idx) {
                        return Ok(());
                    }
                    // 连接提前结束：当作一次可重试的失败。
                    if !progressed {
                        attempts += 1;
                    } else {
                        attempts = 0;
                    }
                    if attempts > self.opts.max_retries {
                        return Err(DownloadError::RetriesExhausted { attempts: attempts - 1, last: "连接反复提前断开".into() });
                    }
                }
                Err(DownloadError::Status(code @ (401 | 403 | 410))) => {
                    // 临时地址可能过期了，回到原始地址刷新一次再试。
                    if !self.refresh_url(seg).await? {
                        return Err(DownloadError::Status(code));
                    }
                }
                Err(e) if e.is_fatal() => return Err(e),
                Err(e) => {
                    attempts = if progressed { 1 } else { attempts + 1 };
                    if attempts > self.opts.max_retries {
                        return Err(DownloadError::RetriesExhausted { attempts: attempts - 1, last: e.to_string() });
                    }
                    self.sleep(backoff(attempts)).await?;
                }
            }
        }
    }

    async fn fetch_range(&self, seg: &SegmentedRun, idx: usize, pos: u64, end: u64) -> Result<()> {
        let url = seg.url.read().unwrap().clone();
        let req = self.request(url).header(header::RANGE, format!("bytes={pos}-{}", end - 1));
        let resp = self.send(req).await?;
        match resp.status() {
            StatusCode::PARTIAL_CONTENT => {}
            StatusCode::OK => return Err(DownloadError::RangeUnsupported),
            s if s == StatusCode::TOO_MANY_REQUESTS || s.is_server_error() => {
                return Err(DownloadError::Network(format!("服务器繁忙 (HTTP {})", s.as_u16())))
            }
            s => return Err(DownloadError::Status(s.as_u16())),
        }
        let h = resp.headers();
        match header_str(h, header::CONTENT_RANGE).and_then(|v| parse_content_range(&v)) {
            Some((start, _, total)) if start == pos && total.is_none_or(|t| t == seg.total) => {}
            Some((_, _, Some(t))) if t != seg.total => return Err(DownloadError::RemoteChanged),
            _ => return Err(DownloadError::Network("服务器返回的分段范围不对".into())),
        }
        if let (Some(a), Some(b)) = (strong_etag(seg.etag.as_deref()), strong_etag(header_str(h, header::ETAG).as_deref())) {
            if a != b {
                return Err(DownloadError::RemoteChanged);
            }
        }

        let mut stream = resp.bytes_stream();
        let mut cursor = pos;
        while let Some(chunk) = self.next_chunk(&mut stream).await? {
            for piece in split_bytes(chunk) {
                self.limiter.acquire(piece.len()).await;
                let allowed = seg.sched.claim(idx, piece.len());
                if allowed == 0 {
                    // 这一段的后半部分已经被别的连接接走了，本连接任务完成。
                    return Ok(());
                }
                let short = allowed < piece.len();
                write_at(seg.file.clone(), cursor, piece.slice(..allowed)).await?;
                seg.sched.commit(idx, allowed);
                self.progress.add(allowed as u64);
                cursor += allowed as u64;
                if short {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// 回原始地址重新获取临时地址。返回是否刷新成功。
    async fn refresh_url(&self, seg: &SegmentedRun) -> Result<bool> {
        let before = seg.url.read().unwrap().clone();
        let _guard = seg.refresh_lock.lock().await;
        // 其他连接已经刷新过了，直接用新的。
        if *seg.url.read().unwrap() != before {
            return Ok(true);
        }
        if seg.refreshes.fetch_add(1, Ordering::Relaxed) >= MAX_URL_REFRESH {
            return Ok(false);
        }
        let info = match self.probe(&seg.original).await {
            Ok(i) => i,
            Err(DownloadError::Cancelled) => return Err(DownloadError::Cancelled),
            Err(_) => return Ok(false),
        };
        if info.total != Some(seg.total) {
            return Err(DownloadError::RemoteChanged);
        }
        *seg.url.write().unwrap() = info.final_url;
        Ok(true)
    }
}

/// 断点处理方案。
enum Plan {
    /// 从头下载。
    Fresh,
    /// 按上次记录的分段续传。
    Segments(Vec<crate::segments::Segment>),
    /// 上次单连接写到了这里，这次改为分段下剩下的部分。
    Convert(u64),
    /// 继续单连接顺序下载。
    Stream(u64),
}

/// 每秒保存一次进度：先生成记录，再刷盘，再写记录。程序被强杀或断电最多损失约 1 秒。
fn spawn_saver(
    file: Arc<File>,
    state_file: PathBuf,
    make: impl Fn() -> ResumeState + Send + Sync + 'static,
) -> tokio::task::JoinHandle<()> {
    let make = Arc::new(make);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.tick().await;
        loop {
            tick.tick().await;
            let (file, path, make) = (file.clone(), state_file.clone(), make.clone());
            let _ = tokio::task::spawn_blocking(move || make().save_durable(&file, &path)).await;
        }
    })
}

struct StreamRun {
    url: Url,
    state_url: String,
    total: Option<u64>,
    /// 中途从头重下时会换成新的 ETag / Last-Modified。
    validators: Mutex<(Option<String>, Option<String>)>,
    /// 已经写入（write_at 返回）的字节数。
    pos: AtomicU64,
    file: Arc<File>,
}

impl StreamRun {
    fn state(&self) -> ResumeState {
        let (etag, lm) = self.validators.lock().unwrap().clone();
        ResumeState::stream(&self.state_url, self.total, etag, lm, self.pos.load(Ordering::SeqCst))
    }
}

impl SegmentedRun {
    fn state(&self) -> ResumeState {
        ResumeState::segmented(&self.state_url, self.total, self.etag.clone(), self.last_modified.clone(), self.sched.snapshot())
    }
}

struct SegmentedRun {
    url: RwLock<Url>,
    original: Url,
    state_url: String,
    refreshes: AtomicU32,
    refresh_lock: tokio::sync::Mutex<()>,
    sched: Scheduler,
    file: Arc<File>,
    total: u64,
    etag: Option<String>,
    last_modified: Option<String>,
}

fn header_str(h: &HeaderMap, name: header::HeaderName) -> Option<String> {
    h.get(name).and_then(|v| v.to_str().ok()).map(|s| s.trim().to_string())
}

fn strong_etag(e: Option<&str>) -> Option<&str> {
    e.filter(|e| !e.starts_with("W/"))
}

/// 解析 `Content-Range`：`bytes 0-99/1000` → (0, 99, Some(1000))；`bytes */1000` → 起止为 0。
fn parse_content_range(v: &str) -> Option<(u64, u64, Option<u64>)> {
    let v = v.trim().strip_prefix("bytes")?.trim();
    let (range, total) = v.split_once('/')?;
    let total = match total.trim() {
        "*" => None,
        t => Some(t.parse().ok()?),
    };
    if range.trim() == "*" {
        return Some((0, 0, total));
    }
    let (a, b) = range.split_once('-')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?, total))
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis((500u64 << attempt.min(5)).min(10_000))
}

fn split_bytes(b: Bytes) -> impl Iterator<Item = Bytes> {
    let len = b.len();
    (0..len).step_by(MAX_CHUNK).map(move |i| b.slice(i..(i + MAX_CHUNK).min(len)))
}

async fn write_at(file: Arc<File>, offset: u64, data: Bytes) -> std::io::Result<()> {
    tokio::task::spawn_blocking(move || write_all_at(&file, &data, offset))
        .await
        .map_err(std::io::Error::other)?
}

fn write_all_at(f: &File, mut buf: &[u8], mut off: u64) -> std::io::Result<()> {
    while !buf.is_empty() {
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_write(f, buf, off)?;
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::write_at(f, buf, off)?;
        if n == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        buf = &buf[n..];
        off += n as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_range() {
        assert_eq!(parse_content_range("bytes 0-0/1000"), Some((0, 0, Some(1000))));
        assert_eq!(parse_content_range("bytes 5-9/*"), Some((5, 9, None)));
        assert_eq!(parse_content_range("bytes */0"), Some((0, 0, Some(0))));
        assert_eq!(parse_content_range("garbage"), None);
    }

    #[test]
    fn splitting() {
        let b = Bytes::from(vec![0u8; MAX_CHUNK * 2 + 5]);
        let lens: Vec<usize> = split_bytes(b).map(|x| x.len()).collect();
        assert_eq!(lens, vec![MAX_CHUNK, MAX_CHUNK, 5]);
    }
}
