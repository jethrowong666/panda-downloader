//! 端到端测试：下载到一半直接强杀 pandadl 进程（相当于任务管理器"结束任务"或程序崩溃），
//! 再用 `pandadl resume` 继续，最终文件必须完全正确，并且只补下剩下的部分。

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;

const MB: usize = 1024 * 1024;

struct Srv {
    data: Bytes,
    sent: AtomicU64,
}

async fn serve(State(s): State<Arc<Srv>>, headers: HeaderMap) -> Response {
    let total = s.data.len();
    let (status, a, b) = match headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("bytes="))
        .and_then(|v| v.split_once('-'))
    {
        Some((a, b)) => {
            let a: usize = a.parse().unwrap();
            let b = if b.is_empty() { total - 1 } else { b.parse::<usize>().unwrap().min(total - 1) };
            (StatusCode::PARTIAL_CONTENT, a, b + 1)
        }
        None => (StatusCode::OK, 0, total),
    };
    let body = s.data.slice(a..b);
    let srv = s.clone();
    // 每 64KB 等 20ms：8 个连接约 25MB/s，足够在中途杀掉进程。
    let stream = futures_util::stream::unfold(0usize, move |off| {
        let body = body.clone();
        let srv = srv.clone();
        async move {
            if off >= body.len() {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            let end = (off + 64 * 1024).min(body.len());
            srv.sent.fetch_add((end - off) as u64, Ordering::SeqCst);
            Some((Ok::<_, std::io::Error>(body.slice(off..end)), end))
        }
    });
    let mut r = Response::builder()
        .status(status)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ETAG, "\"crash-test\"")
        .header(header::CONTENT_LENGTH, b - a);
    if status == StatusCode::PARTIAL_CONTENT {
        r = r.header(header::CONTENT_RANGE, format!("bytes {a}-{}/{total}", b - 1));
    }
    r.body(Body::from_stream(stream)).unwrap()
}

fn test_data(len: usize) -> Bytes {
    let mut x: u64 = 0x9E3779B97F4A7C15;
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

#[tokio::test(flavor = "multi_thread")]
async fn hard_kill_then_resume() {
    let data = test_data(48 * MB);
    let srv = Arc::new(Srv { data: data.clone(), sent: AtomicU64::new(0) });
    let app = Router::new().route("/big.bin", get(serve)).with_state(srv.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let dir = std::env::temp_dir().join(format!("pandadl-crash-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let exe = env!("CARGO_BIN_EXE_pandadl");
    let url = format!("http://{addr}/big.bin");

    // 第一次：下到一半强杀
    let mut child = Command::new(exe)
        .args(["get", &url, "-o", dir.to_str().unwrap(), "-c", "8"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let t0 = Instant::now();
    while srv.sent.load(Ordering::SeqCst) < 20 * MB as u64 {
        assert!(t0.elapsed() < Duration::from_secs(30), "下载太慢");
        assert!(child.try_wait().unwrap().is_none(), "进程提前退出了");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    child.kill().unwrap(); // Windows 上是 TerminateProcess，进程来不及做任何收尾
    child.wait().unwrap();
    let first = srv.sent.load(Ordering::SeqCst);
    assert!(dir.join("big.bin.part").exists());
    assert!(dir.join("big.bin.part.json").exists(), "强杀前没有保存过进度");

    // 第二次：只靠目录里的记录继续
    let out = tokio::task::spawn_blocking({
        let dir = dir.clone();
        move || Command::new(exe).args(["resume", dir.to_str().unwrap()]).output().unwrap()
    })
    .await
    .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "resume 失败:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("找到 1 个未完成的下载"), "{stdout}");

    let got = std::fs::read(dir.join("big.bin")).unwrap();
    assert!(got == data, "强杀后续传的文件内容不对");
    let second = srv.sent.load(Ordering::SeqCst) - first;
    // 强杀最多丢失约 1 秒（这里约 25MB/s）的进度，补下的量应明显少于整个文件。
    assert!(second < (data.len() - 12 * MB) as u64, "续传下载了 {} MB，没有从断点接上", second / MB as u64);
    assert!(!dir.join("big.bin.part").exists() && !dir.join("big.bin.part.json").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
