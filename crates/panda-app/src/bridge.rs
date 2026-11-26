//! 和浏览器扩展的桥：
//! 扩展 →（Native Messaging）→ panda-host.exe →（本机 HTTP，带令牌）→ 这里 → 任务管理器。
//!
//! - 本机接口只监听 127.0.0.1，端口随机；端口和令牌写在 `%APPDATA%\com.panda.downloader\bridge.json`，
//!   只有当前用户能读，panda-host 读它来连接。
//! - 带 Origin 头的请求（网页里的 JS 发来的）一律拒绝，防止网站冒充扩展。
//! - Cookie 只在内存里用，不写进任务记录。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::manager::Manager;

pub const HOST_NAME: &str = "com.panda.downloader";

/// 允许连接的扩展 ID（开发版由 manifest 里的 key 固定；上架商店后把商店分配的 ID 加进来）。
pub const EXTENSION_IDS: &[&str] = &["lmhnncciklhdnekjkjkijjkcmjbcecoo"];

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserCookie {
    pub domain: String,
    pub name: String,
    pub value: String,
    #[serde(default = "root_path")]
    pub path: String,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub expiration_date: Option<f64>,
}

fn root_path() -> String {
    "/".into()
}

/// 浏览器带来的请求上下文。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReqCtx {
    pub referer: Option<String>,
    pub user_agent: Option<String>,
    #[serde(default)]
    pub cookies: Vec<BrowserCookie>,
}

impl ReqCtx {
    /// 适用于这个网址的 Cookie 请求头。
    pub fn cookie_header(&self, url: &str) -> Option<String> {
        let u = url::Url::parse(url).ok()?;
        let host = u.host_str()?.to_ascii_lowercase();
        let path = u.path();
        let https = u.scheme() == "https";
        let pairs: Vec<String> = self
            .cookies
            .iter()
            .filter(|c| {
                let d = c.domain.trim_start_matches('.').to_ascii_lowercase();
                (host == d || host.ends_with(&format!(".{d}"))) && path.starts_with(&c.path) && (https || !c.secure)
            })
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();
        (!pairs.is_empty()).then(|| pairs.join("; "))
    }

    /// Netscape 格式的 Cookie 文件内容（给 yt-dlp 用）。
    pub fn netscape(&self) -> String {
        let mut s = String::from("# Netscape HTTP Cookie File\n");
        for c in &self.cookies {
            let domain = if c.domain.starts_with('.') { c.domain.clone() } else { format!(".{}", c.domain) };
            let exp = c.expiration_date.map(|e| e as i64).unwrap_or(0);
            let clean = |v: &str| v.replace(['\t', '\n', '\r'], "");
            s.push_str(&format!(
                "{domain}\tTRUE\t{}\t{}\t{exp}\t{}\t{}\n",
                clean(&c.path),
                if c.secure { "TRUE" } else { "FALSE" },
                clean(&c.name),
                clean(&c.value)
            ));
        }
        s
    }
}

#[derive(Debug, Deserialize)]
pub struct CaptureItem {
    pub url: String,
    #[serde(default)]
    pub filename: Option<String>,
    /// 扩展嗅探到的视频（m3u8 等），按视频任务处理。
    #[serde(default)]
    pub video: bool,
}

#[derive(Debug, Deserialize)]
struct CaptureReq {
    items: Vec<CaptureItem>,
    #[serde(flatten)]
    ctx: ReqCtx,
}

#[derive(Serialize)]
struct Reply {
    ok: bool,
    count: usize,
    version: &'static str,
}

#[derive(Clone)]
struct AppState {
    m: Arc<Manager>,
    app: tauri::AppHandle,
    token: String,
}

/// 浏览器扩展最近一次来访的时间（Unix 秒，0 = 这次运行还没来过）。
static EXT_SEEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 扩展最近一次连上来是什么时候（Unix 秒）。
pub fn extension_seen() -> Option<u64> {
    match EXT_SEEN.load(std::sync::atomic::Ordering::Relaxed) {
        0 => None,
        t => Some(t),
    }
}

fn authorized(s: &AppState, h: &HeaderMap) -> bool {
    // 网页 JS 发的请求一定带 Origin；panda-host 不带。
    if h.contains_key("origin") {
        return false;
    }
    let ok = h.get("x-panda-token").and_then(|v| v.to_str().ok()).is_some_and(|t| t == s.token);
    if ok {
        EXT_SEEN.store(crate::store::now_secs(), std::sync::atomic::Ordering::Relaxed);
        s.m.note_extension_seen();
    }
    ok
}

async fn ping(State(s): State<AppState>, h: HeaderMap) -> Result<Json<Reply>, StatusCode> {
    if !authorized(&s, &h) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(Json(Reply { ok: true, count: 0, version: env!("CARGO_PKG_VERSION") }))
}

async fn capture(State(s): State<AppState>, h: HeaderMap, Json(req): Json<CaptureReq>) -> Result<Json<Reply>, StatusCode> {
    if !authorized(&s, &h) {
        return Err(StatusCode::FORBIDDEN);
    }
    let n = req.items.len();
    if n > 0 {
        s.m.capture_items(req.items, req.ctx);
    }
    Ok(Json(Reply { ok: true, count: n, version: env!("CARGO_PKG_VERSION") }))
}

async fn show(State(s): State<AppState>, h: HeaderMap) -> Result<Json<Reply>, StatusCode> {
    if !authorized(&s, &h) {
        return Err(StatusCode::FORBIDDEN);
    }
    crate::windows::show_main(&s.app);
    Ok(Json(Reply { ok: true, count: 0, version: env!("CARGO_PKG_VERSION") }))
}

#[derive(Serialize)]
struct BridgeFile {
    port: u16,
    token: String,
    exe: String,
    pid: u32,
}

fn random_token() -> String {
    let mut b = [0u8; 24];
    getrandom::fill(&mut b).expect("系统随机数不可用");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// 启动本机接口，并把端口和令牌写进 bridge.json。
pub async fn serve(m: Arc<Manager>, app: tauri::AppHandle, data_dir: PathBuf) {
    let token = random_token();
    let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else { return };
    let Ok(addr) = listener.local_addr() else { return };
    let file = BridgeFile {
        port: addr.port(),
        token: token.clone(),
        exe: std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
        pid: std::process::id(),
    };
    crate::store::save(&data_dir.join("bridge.json"), &file);
    let state = AppState { m, app, token };
    let router = Router::new()
        .route("/v1/ping", get(ping))
        .route("/v1/capture", post(capture))
        .route("/v1/show", post(show))
        .with_state(state);
    let _ = axum::serve(listener, router).await;
}

/// 给 Chrome 和 Edge 注册 Native Messaging 主机（当前用户，不需要管理员权限）。
pub fn register_native_host(data_dir: &Path) {
    let Ok(exe) = std::env::current_exe() else { return };
    let host = exe.with_file_name("panda-host.exe");
    if !host.exists() {
        return;
    }
    let manifest = serde_json::json!({
        "name": HOST_NAME,
        "description": "熊猫下载器 浏览器扩展通信",
        "path": host.display().to_string(),
        "type": "stdio",
        "allowed_origins": EXTENSION_IDS.iter().map(|id| format!("chrome-extension://{id}/")).collect::<Vec<_>>(),
    });
    let path = data_dir.join("native-host.json");
    crate::store::save(&path, &manifest);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        for browser in [r"Software\Google\Chrome", r"Software\Microsoft\Edge", r"Software\Chromium", r"Software\BraveSoftware\Brave-Browser"] {
            let key = format!(r"HKCU\{browser}\NativeMessagingHosts\{HOST_NAME}");
            let _ = std::process::Command::new("reg")
                .args(["add", &key, "/ve", "/t", "REG_SZ", "/d"])
                .arg(&path)
                .arg("/f")
                .creation_flags(CREATE_NO_WINDOW)
                .status();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ck(domain: &str, name: &str, secure: bool) -> BrowserCookie {
        BrowserCookie { domain: domain.into(), name: name.into(), value: "v".into(), path: "/".into(), secure, expiration_date: None }
    }

    #[test]
    fn cookie_matching() {
        let ctx = ReqCtx { cookies: vec![ck(".douyin.com", "a", false), ck("www.douyin.com", "b", true), ck("other.com", "c", false)], ..Default::default() };
        assert_eq!(ctx.cookie_header("https://www.douyin.com/video/1").as_deref(), Some("a=v; b=v"));
        assert_eq!(ctx.cookie_header("http://www.douyin.com/").as_deref(), Some("a=v"));
        assert_eq!(ctx.cookie_header("https://v.douyin.com/x").as_deref(), Some("a=v"));
        assert_eq!(ctx.cookie_header("https://notdouyin.com/"), None);
    }

    #[test]
    fn netscape_format() {
        let ctx = ReqCtx { cookies: vec![ck("douyin.com", "s_v_web_id", true)], ..Default::default() };
        let s = ctx.netscape();
        assert!(s.contains(".douyin.com\tTRUE\t/\tTRUE\t0\ts_v_web_id\tv"));
    }
}
