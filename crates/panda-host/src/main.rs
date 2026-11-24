//! 熊猫下载器 · 浏览器扩展通信主机（Native Messaging）
//!
//! 浏览器按需启动本程序，通过标准输入输出收发消息（4 字节小端长度 + JSON）。
//! 本程序把消息转给正在运行的熊猫下载器（本机 HTTP + 令牌）；下载器没运行就先在后台启动它。
//! 注意：标准输出只能写协议消息，任何调试输出都会破坏通信。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// 单条消息上限（浏览器规定发给主机的消息最大 4GB，这里限制得小一些防止异常数据）。
const MAX_MSG: usize = 8 * 1024 * 1024;

fn read_msg(r: &mut impl Read) -> Option<Value> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).ok()?;
    let n = u32::from_le_bytes(len) as usize;
    if n == 0 || n > MAX_MSG {
        return None;
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

fn write_msg(w: &mut impl Write, v: &Value) {
    let data = serde_json::to_vec(v).unwrap_or_default();
    let _ = w.write_all(&(data.len() as u32).to_le_bytes());
    let _ = w.write_all(&data);
    let _ = w.flush();
}

fn data_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("com.panda.downloader"))
}

struct Bridge {
    port: u16,
    token: String,
    exe: String,
}

fn load_bridge() -> Option<Bridge> {
    let data = std::fs::read(data_dir()?.join("bridge.json")).ok()?;
    let v: Value = serde_json::from_slice(&data).ok()?;
    Some(Bridge {
        port: v["port"].as_u64()? as u16,
        token: v["token"].as_str()?.to_string(),
        exe: v["exe"].as_str().unwrap_or("").to_string(),
    })
}

/// 极简 HTTP/1.1 请求（只连本机，不需要完整的 HTTP 库）。
fn http(b: &Bridge, method: &str, path: &str, body: &[u8]) -> Result<(u16, Value), String> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], b.port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(15))).ok();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nX-Panda-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        b.port,
        b.token,
        body.len()
    );
    s.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    s.write_all(body).map_err(|e| e.to_string())?;
    let mut resp = Vec::new();
    s.read_to_end(&mut resp).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&resp);
    let status: u16 = text.split(' ').nth(1).and_then(|c| c.parse().ok()).ok_or("响应无效")?;
    let body = text.split_once("\r\n\r\n").map(|x| x.1).unwrap_or("");
    Ok((status, serde_json::from_str(body).unwrap_or(Value::Null)))
}

fn alive(b: &Bridge) -> bool {
    matches!(http(b, "GET", "/v1/ping", b""), Ok((200, _)))
}

/// 拿到可用的连接；下载器没运行就在后台启动它并等它准备好。
fn connect() -> Result<Bridge, String> {
    if let Some(b) = load_bridge() {
        if alive(&b) {
            return Ok(b);
        }
    }
    let exe = load_bridge()
        .map(|b| PathBuf::from(b.exe))
        .filter(|p| p.exists())
        .or_else(|| std::env::current_exe().ok().map(|p| p.with_file_name("PandaDownloader.exe")))
        .filter(|p| p.exists())
        .ok_or("找不到熊猫下载器，请重新安装")?;
    launch_detached(&exe)?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(12) {
        std::thread::sleep(Duration::from_millis(300));
        if let Some(b) = load_bridge() {
            if alive(&b) {
                return Ok(b);
            }
        }
    }
    Err("熊猫下载器启动超时".into())
}

/// 在后台启动下载器，并且不让它继承浏览器和本程序之间的管道：
/// 否则下载器会一直占着管道（浏览器收不到结束信号），它的任何输出也会混进通信消息里。
fn launch_detached(exe: &std::path::Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};
        use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
        unsafe {
            for h in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
                SetHandleInformation(GetStdHandle(h), HANDLE_FLAG_INHERIT, 0);
            }
        }
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--background")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // 浏览器会把扩展主机放进一个"作业"里，主机退出时连带结束子进程；能脱离就脱离。
        if cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB).spawn().is_ok() {
            return Ok(());
        }
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--background")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        return cmd.spawn().map(|_| ()).map_err(|e| format!("无法启动熊猫下载器：{e}"));
    }
    #[cfg(not(windows))]
    std::process::Command::new(exe).arg("--background").spawn().map(|_| ()).map_err(|e| format!("无法启动熊猫下载器：{e}"))
}

fn handle(msg: &Value) -> Value {
    let cmd = msg["cmd"].as_str().unwrap_or("");
    let b = match connect() {
        Ok(b) => b,
        Err(e) => return json!({ "ok": false, "error": e }),
    };
    let r = match cmd {
        "ping" => http(&b, "GET", "/v1/ping", b""),
        "capture" => http(&b, "POST", "/v1/capture", &serde_json::to_vec(&msg["payload"]).unwrap_or_default()),
        "show" => http(&b, "POST", "/v1/show", b"{}"),
        _ => return json!({ "ok": false, "error": "未知命令" }),
    };
    match r {
        Ok((200, v)) => v,
        Ok((code, _)) => json!({ "ok": false, "error": format!("下载器返回 {code}") }),
        Err(e) => json!({ "ok": false, "error": e }),
    }
}

fn main() {
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    // 浏览器关闭输入流时退出；一次连接里可以处理多条消息。
    while let Some(msg) = read_msg(&mut stdin) {
        let reply = handle(&msg);
        write_msg(&mut stdout, &reply);
    }
}
