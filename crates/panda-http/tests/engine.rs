//! HTTP 引擎集成测试：在本机起一个可控的测试服务器，覆盖分段、续传、断线、限速、链接过期等场景。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use panda_http::{ClientConfig, DownloadError, Downloader, RateLimiter, Status, TaskOptions};

const MB: usize = 1024 * 1024;

#[derive(Default)]
struct Cfg {
    ranges: bool,
    /// 不告诉客户端文件大小（Content-Range 写 */，不发 Content-Length）。
    hide_total: bool,
    /// drop_first 也作用于不分段的完整响应。
    drop_full: bool,
    /// 每发送 64KB 等待多久（模拟慢速网络）。
    chunk_delay: Duration,
    /// 只对从 0 开始的请求限速（模拟"某一个连接特别慢"）。
    slow_only_first: bool,
    /// 前 N 个分段请求在发出 1MB 后断开。
    drop_first: usize,
    /// 签名过期：请求数达到这个值后旧签名返回 403。
    rotate_sig_after: usize,
    disposition: Option<String>,
}

struct Srv {
    data: Mutex<Bytes>,
    etag: Mutex<String>,
    cfg: Cfg,
    /// 运行中可切换"是否支持 Range"。
    ranges: AtomicBool,
    requests: AtomicUsize,
    range_requests: AtomicUsize,
    dropped: AtomicUsize,
    bytes_sent: AtomicU64,
    sig: AtomicUsize,
    refreshed: AtomicBool,
}

fn test_data(len: usize, seed: u64) -> Bytes {
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect::<Vec<u8>>()
        .into()
}

async fn start(data: Bytes, cfg: Cfg) -> (Arc<Srv>, String) {
    let srv = Arc::new(Srv {
        data: Mutex::new(data),
        etag: Mutex::new("\"v1\"".into()),
        ranges: AtomicBool::new(cfg.ranges),
        cfg,
        requests: AtomicUsize::new(0),
        range_requests: AtomicUsize::new(0),
        dropped: AtomicUsize::new(0),
        bytes_sent: AtomicU64::new(0),
        sig: AtomicUsize::new(1),
        refreshed: AtomicBool::new(false),
    });
    let app = Router::new()
        .route("/file.bin", get(serve))
        .route("/start", get(redirect))
        .with_state(srv.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (srv, format!("http://{addr}"))
}

#[derive(serde::Deserialize)]
struct SigQuery {
    sig: Option<usize>,
}

/// 模拟 CDN：/start 跳转到带签名的临时地址。
async fn redirect(State(s): State<Arc<Srv>>) -> Response {
    let sig = s.sig.load(Ordering::SeqCst);
    if s.requests.load(Ordering::SeqCst) > 0 {
        s.refreshed.store(true, Ordering::SeqCst);
    }
    (StatusCode::FOUND, [(header::LOCATION, format!("/file.bin?sig={sig}"))]).into_response()
}

async fn serve(State(s): State<Arc<Srv>>, Query(q): Query<SigQuery>, headers: HeaderMap) -> Response {
    let n = s.requests.fetch_add(1, Ordering::SeqCst) + 1;
    if s.cfg.rotate_sig_after > 0 {
        if n == s.cfg.rotate_sig_after {
            s.sig.fetch_add(1, Ordering::SeqCst);
        }
        if q.sig != Some(s.sig.load(Ordering::SeqCst)) {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    let data = s.data.lock().unwrap().clone();
    let etag = s.etag.lock().unwrap().clone();
    let total = data.len();

    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| v.split_once('-'))
        .map(|(a, b)| (a.parse::<usize>().unwrap(), if b.is_empty() { total - 1 } else { b.parse::<usize>().unwrap() }));

    let ranges_on = s.ranges.load(Ordering::SeqCst);
    let mut builder = Response::builder().header(header::ETAG, &etag).header(header::ACCEPT_RANGES, if ranges_on { "bytes" } else { "none" });
    if let Some(d) = &s.cfg.disposition {
        builder = builder.header(header::CONTENT_DISPOSITION, d);
    }

    let (status, start, end) = match range {
        Some((a, b)) if ranges_on => {
            if total == 0 || a >= total {
                return builder
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{total}"))
                    .body(Body::empty())
                    .unwrap();
            }
            let b = b.min(total - 1);
            let shown = if s.cfg.hide_total { "*".to_string() } else { total.to_string() };
            builder = builder.header(header::CONTENT_RANGE, format!("bytes {a}-{b}/{shown}"));
            (StatusCode::PARTIAL_CONTENT, a, b + 1)
        }
        _ => (StatusCode::OK, 0, total),
    };
    let is_probe = end - start == 1;
    if status == StatusCode::PARTIAL_CONTENT && !is_probe {
        s.range_requests.fetch_add(1, Ordering::SeqCst);
    }
    let droppable = (status == StatusCode::PARTIAL_CONTENT && !is_probe) || (status == StatusCode::OK && s.cfg.drop_full);
    let drop_after = if droppable && s.dropped.load(Ordering::SeqCst) < s.cfg.drop_first {
        s.dropped.fetch_add(1, Ordering::SeqCst);
        Some(MB)
    } else {
        None
    };
    let delay = if s.cfg.slow_only_first && start != 0 { Duration::ZERO } else { s.cfg.chunk_delay };

    let body = data.slice(start..end);
    let srv = s.clone();
    let stream = async_stream(body, delay, drop_after, srv);
    if !s.cfg.hide_total {
        builder = builder.header(header::CONTENT_LENGTH, end - start);
    }
    builder.status(status).body(Body::from_stream(stream)).unwrap()
}

fn async_stream(
    body: Bytes,
    delay: Duration,
    drop_after: Option<usize>,
    srv: Arc<Srv>,
) -> impl futures_util::Stream<Item = Result<Bytes, std::io::Error>> {
    const CHUNK: usize = 64 * 1024;
    futures_util::stream::unfold(0usize, move |off| {
        let body = body.clone();
        let srv = srv.clone();
        async move {
            if off >= body.len() {
                return None;
            }
            if let Some(limit) = drop_after {
                if off >= limit {
                    return Some((Err(std::io::Error::other("模拟断线")), usize::MAX));
                }
            }
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let end = (off + CHUNK).min(body.len());
            srv.bytes_sent.fetch_add((end - off) as u64, Ordering::SeqCst);
            Some((Ok(body.slice(off..end)), end))
        }
    })
}

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("panda-http-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn opts(dir: &Path, connections: usize) -> TaskOptions {
    TaskOptions { connections, out_dir: dir.to_path_buf(), read_timeout: Duration::from_secs(5), ..Default::default() }
}

fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".part"))
        .collect()
}

fn dl() -> Downloader {
    Downloader::new(&ClientConfig::default()).unwrap()
}

#[tokio::test]
async fn segmented_download_is_byte_exact() {
    let data = test_data(20 * MB + 12345, 1);
    let (srv, base) = start(data.clone(), Cfg { ranges: true, ..Default::default() }).await;
    let dir = tmpdir("seg");
    let task = dl().start(&format!("{base}/file.bin"), opts(&dir, 8));
    let progress = task.progress.clone();
    let path = task.wait().await.unwrap();
    assert_eq!(path.file_name().unwrap(), "file.bin");
    assert!(std::fs::read(&path).unwrap() == data, "内容不一致");
    assert!(srv.range_requests.load(Ordering::SeqCst) >= 8);
    assert_eq!(progress.downloaded(), data.len() as u64);
    assert_eq!(progress.status(), Status::Done);
    assert!(leftovers(&dir).is_empty(), "残留 {:?}", leftovers(&dir));
}

#[tokio::test]
async fn falls_back_to_single_connection_without_ranges() {
    let data = test_data(3 * MB, 2);
    let (srv, base) = start(data.clone(), Cfg { ranges: false, ..Default::default() }).await;
    let dir = tmpdir("single");
    let task = dl().start(&format!("{base}/file.bin"), opts(&dir, 8));
    let progress = task.progress.clone();
    let path = task.wait().await.unwrap();
    assert!(std::fs::read(&path).unwrap() == data);
    assert!(!progress.resumable());
    // 探测请求本身就被复用为下载请求，只发 1 次。
    assert_eq!(srv.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn recovers_from_dropped_connections() {
    let data = test_data(12 * MB, 3);
    let (srv, base) = start(data.clone(), Cfg { ranges: true, drop_first: 6, ..Default::default() }).await;
    let dir = tmpdir("drop");
    let path = dl().start(&format!("{base}/file.bin"), opts(&dir, 4)).wait().await.unwrap();
    assert!(std::fs::read(&path).unwrap() == data);
    assert_eq!(srv.dropped.load(Ordering::SeqCst), 6);
}

#[tokio::test]
async fn pause_and_resume_downloads_only_the_rest() {
    let data = test_data(16 * MB, 4);
    // 每 64KB 等 10ms，8 个连接约 50MB/s 以下，足够在中途暂停。
    let (srv, base) =
        start(data.clone(), Cfg { ranges: true, chunk_delay: Duration::from_millis(10), ..Default::default() }).await;
    let dir = tmpdir("resume");
    let url = format!("{base}/file.bin");
    let d = dl();

    let task = d.start(&url, opts(&dir, 8));
    let p = task.progress.clone();
    while p.downloaded() < 4 * MB as u64 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    task.pause();
    let r = task.wait().await;
    assert!(matches!(r, Err(DownloadError::Cancelled)), "{r:?}");
    assert_eq!(p.status(), Status::Paused);
    let first_run = srv.bytes_sent.load(Ordering::SeqCst);
    assert!(dir.join("file.bin.part").exists() && dir.join("file.bin.part.json").exists());

    let task = d.start(&url, opts(&dir, 8));
    let p2 = task.progress.clone();
    let path = task.wait().await.unwrap();
    assert_eq!(path, dir.join("file.bin"));
    assert!(std::fs::read(&path).unwrap() == data, "续传后内容不一致");
    let second_run = srv.bytes_sent.load(Ordering::SeqCst) - first_run;
    // 第二次只下载剩余部分（允许暂停时少量在途数据被重下）。
    assert!(second_run < data.len() as u64 - 3 * MB as u64, "第二次下载了 {second_run} 字节");
    assert_eq!(p2.downloaded(), data.len() as u64);
    assert!(leftovers(&dir).is_empty());
}

#[tokio::test]
async fn changed_remote_file_restarts_cleanly() {
    let data = test_data(8 * MB, 5);
    let (srv, base) =
        start(data.clone(), Cfg { ranges: true, chunk_delay: Duration::from_millis(10), ..Default::default() }).await;
    let dir = tmpdir("changed");
    let url = format!("{base}/file.bin");
    let d = dl();
    let task = d.start(&url, opts(&dir, 4));
    let p = task.progress.clone();
    while p.downloaded() < MB as u64 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    task.pause();
    let _ = task.wait().await;

    // 服务器换了文件
    let new_data = test_data(8 * MB, 99);
    *srv.data.lock().unwrap() = new_data.clone();
    *srv.etag.lock().unwrap() = "\"v2\"".into();

    let path = d.start(&url, opts(&dir, 4)).wait().await.unwrap();
    assert_eq!(path, dir.join("file.bin"), "应沿用原文件名");
    assert!(std::fs::read(&path).unwrap() == new_data, "应是新文件内容");
    assert!(leftovers(&dir).is_empty());
}

#[tokio::test]
async fn slow_connection_gets_split_dynamically() {
    let data = test_data(16 * MB, 6);
    // 只有从 0 开始的那个连接很慢（每 64KB 等 40ms，约 1.6MB/s）
    let (srv, base) = start(
        data.clone(),
        Cfg { ranges: true, chunk_delay: Duration::from_millis(40), slow_only_first: true, ..Default::default() },
    )
    .await;
    let dir = tmpdir("split");
    let t0 = Instant::now();
    let o = TaskOptions { min_split: 512 * 1024, ..opts(&dir, 4) };
    let path = dl().start(&format!("{base}/file.bin"), o).wait().await.unwrap();
    let secs = t0.elapsed().as_secs_f64();
    assert!(std::fs::read(&path).unwrap() == data);
    // 不劈段的话，慢连接独自下 4MB 需要约 2.6 秒；劈段后其他连接会接走大部分。
    assert!(srv.range_requests.load(Ordering::SeqCst) > 4, "没有发生动态劈段");
    assert!(secs < 1.5, "用时 {secs:.2}s，动态劈段没有生效");
}

#[tokio::test]
async fn expired_signed_url_is_refreshed() {
    let data = test_data(10 * MB, 7);
    let (srv, base) = start(
        data.clone(),
        Cfg { ranges: true, rotate_sig_after: 4, chunk_delay: Duration::from_millis(2), ..Default::default() },
    )
    .await;
    let dir = tmpdir("refresh");
    let path = dl().start(&format!("{base}/start"), opts(&dir, 6)).wait().await.unwrap();
    assert!(std::fs::read(&path).unwrap() == data);
    assert!(srv.refreshed.load(Ordering::SeqCst), "没有回原始地址刷新");
}

#[tokio::test]
async fn name_from_content_disposition() {
    let data = test_data(1000, 8);
    let (_srv, base) = start(
        data.clone(),
        Cfg {
            ranges: true,
            disposition: Some("attachment; filename=\"x.bin\"; filename*=UTF-8''%E7%86%8A%E7%8C%AB.zip".into()),
            ..Default::default()
        },
    )
    .await;
    let dir = tmpdir("name");
    let path = dl().start(&format!("{base}/file.bin"), opts(&dir, 4)).wait().await.unwrap();
    assert_eq!(path.file_name().unwrap(), "熊猫.zip");
    // 再下一次不会覆盖，而是自动改名
    let path2 = dl().start(&format!("{base}/file.bin"), opts(&dir, 4)).wait().await.unwrap();
    assert_eq!(path2.file_name().unwrap(), "熊猫 (1).zip");
}

#[tokio::test]
async fn empty_file() {
    let (_srv, base) = start(Bytes::new(), Cfg { ranges: true, ..Default::default() }).await;
    let dir = tmpdir("empty");
    let path = dl().start(&format!("{base}/file.bin"), opts(&dir, 4)).wait().await.unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
}

#[tokio::test]
async fn speed_limit_is_respected() {
    let data = test_data(3 * MB, 9);
    let (_srv, base) = start(data.clone(), Cfg { ranges: true, ..Default::default() }).await;
    let dir = tmpdir("limit");
    let limiter = Arc::new(RateLimiter::new(2 * MB as u64));
    let d = Downloader::with_limiter(&ClientConfig::default(), limiter).unwrap();
    let t0 = Instant::now();
    let path = d.start(&format!("{base}/file.bin"), opts(&dir, 8)).wait().await.unwrap();
    let secs = t0.elapsed().as_secs_f64();
    assert!(std::fs::read(&path).unwrap() == data);
    assert!(secs > 1.2 && secs < 2.5, "3MB 以 2MB/s 下载用了 {secs:.2}s");
}

#[tokio::test]
async fn missing_file_fails_fast() {
    let (_srv, base) = start(test_data(10, 1), Cfg { ranges: true, ..Default::default() }).await;
    let dir = tmpdir("404");
    let t0 = Instant::now();
    let r = dl().start(&format!("{base}/nope"), opts(&dir, 4)).wait().await;
    assert!(matches!(r, Err(DownloadError::Status(404))), "{r:?}");
    assert!(t0.elapsed() < Duration::from_secs(2), "404 不应该重试");
}

// ───────────────────────── 断点续传专项 ─────────────────────────

/// 跑到下载量超过 `at` 字节时暂停，返回暂停时服务器已发出的字节数。
async fn pause_at(d: &Downloader, url: &str, o: TaskOptions, at: u64, srv: &Srv) -> u64 {
    let task = d.start(url, o);
    let p = task.progress.clone();
    while p.downloaded() < at {
        assert!(!task.is_finished(), "还没暂停就结束了: {:?}", p.status());
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    task.pause();
    let r = task.wait().await;
    assert!(matches!(r, Err(DownloadError::Cancelled)), "{r:?}");
    srv.bytes_sent.load(Ordering::SeqCst)
}

#[tokio::test]
async fn saved_state_never_claims_unwritten_data() {
    // 暂停后检查：记录里"已完成"的每一个字节，在 .part 文件里都必须是正确数据。
    let data = test_data(16 * MB, 10);
    let (srv, base) =
        start(data.clone(), Cfg { ranges: true, chunk_delay: Duration::from_millis(5), ..Default::default() }).await;
    let dir = tmpdir("consistency");
    pause_at(&dl(), &format!("{base}/file.bin"), opts(&dir, 8), 5 * MB as u64, &srv).await;
    let st = panda_http::state::ResumeState::load(&dir.join("file.bin.part.json")).unwrap();
    let part = std::fs::read(dir.join("file.bin.part")).unwrap();
    assert_eq!(part.len(), data.len());
    let mut pending = vec![false; data.len()];
    for s in &st.segments {
        pending[s.pos as usize..s.end as usize].iter_mut().for_each(|x| *x = true);
    }
    let claimed = pending.iter().filter(|x| !**x).count();
    assert!(claimed >= 5 * MB, "记录里只有 {claimed} 字节已完成");
    for (i, (&pend, (&a, &b))) in pending.iter().zip(part.iter().zip(data.iter())).enumerate() {
        assert!(pend || a == b, "第 {i} 字节记录为已完成，但数据不对");
    }
}

#[tokio::test]
async fn unknown_size_pause_and_resume() {
    // 服务器支持 Range 但不告诉文件大小（动态生成的文件常见）。
    let data = test_data(8 * MB, 11);
    let (srv, base) = start(
        data.clone(),
        Cfg { ranges: true, hide_total: true, chunk_delay: Duration::from_millis(5), ..Default::default() },
    )
    .await;
    let dir = tmpdir("unknown-size");
    let url = format!("{base}/file.bin");
    let d = dl();
    let first = pause_at(&d, &url, opts(&dir, 8), 3 * MB as u64, &srv).await;
    let st = panda_http::state::ResumeState::load(&dir.join("file.bin.part.json")).unwrap();
    assert!(st.stream && st.total.is_none());

    let task = d.start(&url, opts(&dir, 8));
    let p = task.progress.clone();
    let path = task.wait().await.unwrap();
    assert!(p.resumable());
    assert!(std::fs::read(&path).unwrap() == data, "续传后内容不一致");
    let second = srv.bytes_sent.load(Ordering::SeqCst) - first;
    assert!(second < data.len() as u64 - 2 * MB as u64, "第二次下载了 {second} 字节，没有续传");
    assert!(leftovers(&dir).is_empty());
}

#[tokio::test]
async fn unknown_size_reconnects_without_starting_over() {
    let data = test_data(6 * MB, 12);
    let (srv, base) =
        start(data.clone(), Cfg { ranges: true, hide_total: true, drop_first: 3, drop_full: true, ..Default::default() })
            .await;
    let dir = tmpdir("unknown-drop");
    let path = dl().start(&format!("{base}/file.bin"), opts(&dir, 8)).wait().await.unwrap();
    assert!(std::fs::read(&path).unwrap() == data);
    assert_eq!(srv.dropped.load(Ordering::SeqCst), 3);
    // 每次断线都从断点接着下，总发送量应接近文件大小，而不是重下多遍。
    let sent = srv.bytes_sent.load(Ordering::SeqCst);
    assert!(sent < data.len() as u64 + MB as u64, "发送了 {sent} 字节");
}

#[tokio::test]
async fn no_range_server_survives_dropped_connection() {
    // 服务器完全不支持 Range：断线后只能从头下载，但不能直接失败。
    let data = test_data(4 * MB, 13);
    let (srv, base) =
        start(data.clone(), Cfg { ranges: false, drop_first: 2, drop_full: true, ..Default::default() }).await;
    let dir = tmpdir("norange-drop");
    let path = dl().start(&format!("{base}/file.bin"), opts(&dir, 8)).wait().await.unwrap();
    assert!(std::fs::read(&path).unwrap() == data);
    assert_eq!(srv.dropped.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn no_range_server_pause_keeps_file_and_finishes() {
    let data = test_data(6 * MB, 14);
    let (srv, base) =
        start(data.clone(), Cfg { ranges: false, chunk_delay: Duration::from_millis(5), ..Default::default() }).await;
    let dir = tmpdir("norange-pause");
    let url = format!("{base}/file.bin");
    let d = dl();
    pause_at(&d, &url, opts(&dir, 8), 2 * MB as u64, &srv).await;
    assert!(dir.join("file.bin.part").exists(), "暂停不应删除已下载的数据");
    assert!(dir.join("file.bin.part.json").exists());
    let path = d.start(&url, opts(&dir, 8)).wait().await.unwrap();
    assert_eq!(path, dir.join("file.bin"));
    assert!(std::fs::read(&path).unwrap() == data);
    assert!(leftovers(&dir).is_empty());
}

#[tokio::test]
async fn single_connection_progress_upgrades_to_segmented() {
    // 第一次服务器（或 CDN 节点）不支持 Range，下了一半暂停；第二次支持了：保留已下的部分，剩下的分段下。
    let data = test_data(12 * MB, 15);
    let (srv, base) =
        start(data.clone(), Cfg { ranges: false, chunk_delay: Duration::from_millis(5), ..Default::default() }).await;
    let dir = tmpdir("upgrade");
    let url = format!("{base}/file.bin");
    let d = dl();
    let first = pause_at(&d, &url, opts(&dir, 8), 4 * MB as u64, &srv).await;

    srv.ranges.store(true, Ordering::SeqCst);
    let path = d.start(&url, opts(&dir, 8)).wait().await.unwrap();
    assert!(std::fs::read(&path).unwrap() == data, "升级为分段后内容不一致");
    assert!(srv.range_requests.load(Ordering::SeqCst) > 1, "没有改用分段下载");
    let second = srv.bytes_sent.load(Ordering::SeqCst) - first;
    assert!(second < data.len() as u64 - 3 * MB as u64, "第二次下载了 {second} 字节，没有保留已下的部分");
}

#[tokio::test]
async fn same_name_from_other_url_does_not_destroy_paused_download() {
    let data_a = test_data(8 * MB, 16);
    let (srv_a, base_a) =
        start(data_a.clone(), Cfg { ranges: true, chunk_delay: Duration::from_millis(5), ..Default::default() }).await;
    let dir = tmpdir("same-name");
    let d = dl();
    pause_at(&d, &format!("{base_a}/file.bin"), opts(&dir, 4), MB as u64, &srv_a).await;

    // 另一个网站上同名但不同的文件
    let data_b = test_data(3 * MB, 17);
    let (_srv_b, base_b) = start(data_b.clone(), Cfg { ranges: true, ..Default::default() }).await;
    let pb = d.start(&format!("{base_b}/file.bin"), opts(&dir, 4)).wait().await.unwrap();
    assert_eq!(pb.file_name().unwrap(), "file (1).bin");
    assert!(std::fs::read(&pb).unwrap() == data_b);
    assert!(dir.join("file.bin.part.json").exists(), "A 的断点记录被破坏了");

    // A 仍然可以续传
    let pa = d.start(&format!("{base_a}/file.bin"), opts(&dir, 4)).wait().await.unwrap();
    assert_eq!(pa.file_name().unwrap(), "file.bin");
    assert!(std::fs::read(&pa).unwrap() == data_a);
}

#[tokio::test]
async fn resume_from_saved_record_with_new_link() {
    // 只靠 .part.json 里记下的地址就能继续（pandadl resume 的做法），
    // 而且换一个新复制的链接（比如带了新签名参数）也能接上同一个文件。
    let data = test_data(8 * MB, 18);
    let (srv, base) =
        start(data.clone(), Cfg { ranges: true, chunk_delay: Duration::from_millis(5), ..Default::default() }).await;
    let dir = tmpdir("resume-record");
    let d = dl();
    pause_at(&d, &format!("{base}/file.bin"), opts(&dir, 4), 2 * MB as u64, &srv).await;

    let list = panda_http::state::list_unfinished(&dir);
    assert_eq!(list.len(), 1);
    let (name, st) = &list[0];
    assert_eq!(name, "file.bin");
    assert_eq!(st.url, format!("{base}/file.bin"));

    let new_link = format!("{base}/file.bin?token=fresh");
    let o = TaskOptions { file_name: Some(name.clone()), ..opts(&dir, 4) };
    let path = d.start(&new_link, o).wait().await.unwrap();
    assert_eq!(path, dir.join("file.bin"));
    assert!(std::fs::read(&path).unwrap() == data);
    assert!(panda_http::state::list_unfinished(&dir).is_empty());
}
