//! 熊猫下载器命令行版（pandadl）。
//!
//! ```text
//! pandadl get <链接…> [-o 目录] [-c 连接数] [-l 限速]
//! pandadl parse <链接…> [--json]
//! pandadl extract <文本文件 | ->
//! ```

use std::io::{IsTerminal, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use panda_http::{ClientConfig, DownloadError, Downloader, RateLimiter, Status, TaskOptions};
use panda_link::{batch, Engine, LinkKind};

#[derive(Parser)]
#[command(name = "pandadl", version, about = "熊猫下载器 Panda Downloader · 命令行版")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 下载。支持 http(s) 直链，以及解包后是直链的迅雷/快车/旋风链接；支持批量模式 [001-100]
    Get {
        links: Vec<String>,
        /// 保存目录
        #[arg(short, long, default_value = ".")]
        out: PathBuf,
        /// 指定文件名（只下载一个文件时有效）
        #[arg(short = 'n', long)]
        name: Option<String>,
        #[command(flatten)]
        net: NetArgs,
    },
    /// 继续目录里所有未完成的下载（不用再输入原链接）
    Resume {
        /// 下载目录
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// 只列出未完成的任务，不下载
        #[arg(long)]
        list: bool,
        #[command(flatten)]
        net: NetArgs,
    },
    /// 识别链接类型，显示解包结果和该用哪个引擎
    Parse {
        links: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// 从一段文字里提取所有下载链接（"-" 表示从标准输入读）
    Extract { file: String },
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("错误: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Parse { links, json } => cmd_parse(&links, json),
        Cmd::Extract { file } => {
            let mut text = String::new();
            if file == "-" {
                std::io::stdin().read_to_string(&mut text)?;
            } else {
                text = String::from_utf8_lossy(&std::fs::read(&file).with_context(|| format!("读取 {file}"))?).into_owned();
            }
            for l in panda_link::extract_links(&text) {
                println!("{l}");
            }
            Ok(())
        }
        Cmd::Get { links, out, name, net } => {
            let mut expanded = Vec::new();
            for l in &links {
                expanded.extend(batch::expand(l)?);
            }
            if expanded.is_empty() {
                bail!("没有给出链接");
            }
            if name.is_some() && expanded.len() > 1 {
                bail!("--name 只能用于单个文件");
            }
            let (dl, base) = net.build(out)?;
            let jobs = expanded.into_iter().map(|l| (l, name.clone())).collect();
            run_jobs(&dl, &base, jobs).await
        }
        Cmd::Resume { dir, list, net } => {
            let unfinished = panda_http::state::list_unfinished(&dir);
            if unfinished.is_empty() {
                println!("{} 里没有未完成的下载。", dir.display());
                return Ok(());
            }
            println!("找到 {} 个未完成的下载：", unfinished.len());
            for (name, st) in &unfinished {
                let done = if st.stream {
                    st.stream_pos()
                } else {
                    st.total.unwrap_or(0) - st.segments.iter().map(|s| s.remaining()).sum::<u64>()
                };
                let pct = match st.total {
                    Some(t) if t > 0 => format!("{:.1}%", done as f64 * 100.0 / t as f64),
                    _ => human(done),
                };
                println!("  {name}  {pct}  {}", st.url);
            }
            if list {
                return Ok(());
            }
            let (dl, base) = net.build(dir)?;
            // 指定原文件名，保证接上的是同一个 .part 文件。
            let jobs = unfinished.into_iter().map(|(name, st)| (st.url, Some(name))).collect();
            run_jobs(&dl, &base, jobs).await
        }
    }
}

/// 下载和续传共用的网络参数。
#[derive(clap::Args)]
struct NetArgs {
    /// 最大连接数
    #[arg(short, long, default_value_t = 8)]
    connections: usize,
    /// 限速，例如 500K、2M、1.5MB（每秒）
    #[arg(short, long)]
    limit: Option<String>,
    #[arg(long)]
    referer: Option<String>,
    #[arg(long)]
    cookie: Option<String>,
    #[arg(long)]
    user_agent: Option<String>,
    /// 自定义请求头，可重复："Name: Value"
    #[arg(short = 'H', long = "header")]
    headers: Vec<String>,
    /// 代理，例如 http://127.0.0.1:7890 或 socks5://127.0.0.1:1080
    #[arg(long)]
    proxy: Option<String>,
}

impl NetArgs {
    fn build(self, out_dir: PathBuf) -> Result<(Downloader, TaskOptions)> {
        let rate = self.limit.as_deref().map(parse_size).transpose()?.unwrap_or(0);
        let limiter = Arc::new(RateLimiter::new(rate));
        let dl = Downloader::with_limiter(&ClientConfig { proxy: self.proxy, ..Default::default() }, limiter)?;
        let headers = self
            .headers
            .iter()
            .map(|h| h.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).context("请求头格式应为 \"Name: Value\""))
            .collect::<Result<Vec<_>>>()?;
        let opts = TaskOptions {
            connections: self.connections,
            out_dir,
            referer: self.referer,
            cookie: self.cookie,
            user_agent: self.user_agent,
            headers,
            ..Default::default()
        };
        Ok((dl, opts))
    }
}

/// 依次下载；按 Ctrl+C 时暂停当前任务并退出。
async fn run_jobs(dl: &Downloader, base: &TaskOptions, jobs: Vec<(String, Option<String>)>) -> Result<()> {
    let n = jobs.len();
    let mut failed = 0;
    for (i, (link, name)) in jobs.into_iter().enumerate() {
        if n > 1 {
            println!("[{}/{}] {link}", i + 1, n);
        }
        let opts = TaskOptions { file_name: name, ..base.clone() };
        match download_one(dl, &link, opts).await {
            Ok(true) => {}
            Ok(false) => return Ok(()),
            Err(e) => {
                eprintln!("  失败: {e:#}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        bail!("{failed} 个任务失败");
    }
    Ok(())
}

fn cmd_parse(links: &[String], json: bool) -> Result<()> {
    for l in links {
        match panda_link::parse(l) {
            Ok(p) => {
                if json {
                    println!("{}", serde_json::to_string_pretty(&p)?);
                    continue;
                }
                let e = p.engine();
                println!("输入: {}", p.input);
                if !p.wrappers.is_empty() {
                    println!("解包: {:?} → {}", p.wrappers, p.resolved);
                }
                println!("类型: {}", describe(&p.kind));
                println!("引擎: {}{}", e.label(), if e.implemented() { "" } else { "（后续版本支持）" });
                println!();
            }
            Err(e) => println!("输入: {l}\n错误: {e}\n"),
        }
    }
    Ok(())
}

fn describe(k: &LinkKind) -> String {
    match k {
        LinkKind::Http { .. } => "HTTP 直链".into(),
        LinkKind::Ftp { .. } => "FTP".into(),
        LinkKind::Sftp { .. } => "SFTP".into(),
        LinkKind::Magnet(m) => {
            let mut s = String::from("磁力链接");
            if let Some(h) = &m.btih {
                s += &format!(" btih={h}");
            }
            if let Some(h) = &m.ed2k {
                s += &format!(" ed2k={h}");
            }
            if let Some(n) = &m.name {
                s += &format!(" 名称={n}");
            }
            if !m.trackers.is_empty() {
                s += &format!(" tracker×{}", m.trackers.len());
            }
            s
        }
        LinkKind::Ed2k(panda_link::Ed2kLink::File(f)) => {
            format!("电驴文件 {}（{}）MD4={}", f.name, human(f.size), f.hash)
        }
        LinkKind::Ed2k(panda_link::Ed2kLink::Server { host, port }) => format!("电驴服务器 {host}:{port}"),
        LinkKind::Ed2k(panda_link::Ed2kLink::ServerList { url }) => format!("电驴服务器列表 {url}"),
        LinkKind::Torrent { location } => format!("种子文件 {location}"),
        LinkKind::Metalink { location } => format!("Metalink {location}"),
        LinkKind::Hls { .. } => "HLS 流媒体 (m3u8)".into(),
        LinkKind::Dash { .. } => "DASH 流媒体 (mpd)".into(),
        LinkKind::Stream { protocol, .. } => format!("{} 流", protocol.to_uppercase()),
        LinkKind::Ipfs { namespace, path } => format!("{} {path}", namespace.to_uppercase()),
        LinkKind::Unknown { .. } => "无法识别".into(),
    }
}

/// 返回 Ok(false) 表示用户暂停。
async fn download_one(dl: &Downloader, link: &str, opts: TaskOptions) -> Result<bool> {
    let parsed = panda_link::parse(link)?;
    if !parsed.wrappers.is_empty() {
        println!("  解包 {:?} → {}", parsed.wrappers, parsed.resolved);
    }
    let engine = parsed.engine();
    if engine != Engine::Http {
        bail!("{}（{}）将在后续版本支持", describe(&parsed.kind), engine.label());
    }

    let task = dl.start(&parsed.resolved, opts);
    let progress = task.progress.clone();
    let tty = std::io::stdout().is_terminal();
    let mut started = Instant::now();
    let mut resumed_from: Option<u64> = None;
    let mut samples: std::collections::VecDeque<(Instant, u64)> = Default::default();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut paused = false;
    let mut last_print = Instant::now() - Duration::from_secs(10);

    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);
    while !task.is_finished() {
        tokio::select! {
            _ = &mut ctrl_c, if !paused => {
                paused = true;
                task.pause();
            }
            _ = tick.tick() => {
                // 探测阶段不计速度，否则续传时"已下载的部分"会被算成瞬间速度。
                if progress.status() != Status::Downloading { continue; }
                let now = Instant::now();
                let done = progress.downloaded();
                if resumed_from.is_none() {
                    resumed_from = Some(done);
                    started = now;
                    if done > 0 {
                        let pct = progress.total().filter(|t| *t > 0).map(|t| format!("{:.1}%", done as f64 * 100.0 / t as f64));
                        let line = format!("  从断点继续：已下载 {}{}", human(done), pct.map(|p| format!("（{p}）")).unwrap_or_default());
                        if tty { println!("\r{line:<90}"); } else { println!("{line}"); }
                    }
                }
                samples.push_back((now, done));
                while samples.len() > 10 { samples.pop_front(); } // 最近 5 秒的平均速度
                let speed = match (samples.front(), samples.back()) {
                    (Some(a), Some(b)) if b.0 > a.0 => b.1.saturating_sub(a.1) as f64 / (b.0 - a.0).as_secs_f64(),
                    _ => 0.0,
                };
                // 非终端（例如输出到文件）时每 5 秒打印一行。
                if !tty && now.duration_since(last_print) < Duration::from_secs(5) { continue; }
                last_print = now;
                let line = progress_line(&progress, speed);
                if tty { print!("\r{line:<90}"); } else { println!("{line}"); }
                std::io::stdout().flush().ok();
            }
        }
    }
    let name = progress.path().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
    let resumable = progress.resumable();
    let result = task.wait().await;
    if tty {
        print!("\r{:<90}\r", "");
    }
    match result {
        Ok(path) => {
            let secs = started.elapsed().as_secs_f64().max(0.001);
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            // 只按本次实际下载的量算平均速度。
            let this_run = size.saturating_sub(resumed_from.unwrap_or(0));
            let resumed = if this_run < size { format!("，本次续传 {}", human(this_run)) } else { String::new() };
            println!(
                "  完成 {}（{}{resumed}，用时 {:.1} 秒，平均 {}/s）",
                path.display(),
                human(size),
                secs,
                human((this_run as f64 / secs) as u64)
            );
            Ok(true)
        }
        Err(DownloadError::Cancelled) => {
            println!("  已暂停 {name}，进度已保存。继续下载：pandadl resume <目录>，或再次运行同样的命令。");
            if !resumable {
                println!("  注意：这个服务器不支持断点续传，继续时会先尝试续传，服务器不认的话只能从头下载。");
            }
            Ok(false)
        }
        Err(e) => Err(e.into()),
    }
}

fn progress_line(p: &panda_http::Progress, speed: f64) -> String {
    let done = p.downloaded();
    let name = p.path().and_then(|x| x.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
    let name: String = if name.chars().count() > 28 { name.chars().take(27).chain(['…']).collect() } else { name };
    match p.total() {
        Some(t) if t > 0 => {
            let pct = done as f64 * 100.0 / t as f64;
            let eta = if speed > 1.0 { fmt_eta(((t - done.min(t)) as f64 / speed) as u64) } else { "--:--".into() };
            format!("  {name} {pct:5.1}% {}/{} {}/s {}连接 剩余{eta}", human(done), human(t), human(speed as u64), p.connections())
        }
        _ => format!("  {name} {} {}/s", human(done), human(speed as u64)),
    }
}

fn fmt_eta(s: u64) -> String {
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

fn human(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{n}B") } else { format!("{v:.1}{}", U[i]) }
}

/// "500K" / "2M" / "1.5MB" / "800" → 字节数。
fn parse_size(s: &str) -> Result<u64> {
    let t = s.trim().to_ascii_uppercase();
    let t = t.trim_end_matches("/S").trim_end_matches('B');
    let (num, mul) = match t.chars().last() {
        Some('K') => (&t[..t.len() - 1], 1024.0),
        Some('M') => (&t[..t.len() - 1], 1024.0 * 1024.0),
        Some('G') => (&t[..t.len() - 1], 1024.0 * 1024.0 * 1024.0),
        _ => (t, 1.0),
    };
    let v: f64 = num.trim().parse().with_context(|| format!("限速格式不对: {s}"))?;
    Ok((v * mul) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("500K").unwrap(), 512_000);
        assert_eq!(parse_size("2M").unwrap(), 2 * 1024 * 1024);
        assert_eq!(parse_size("1.5MB").unwrap(), 1_572_864);
        assert_eq!(parse_size("800").unwrap(), 800);
        assert!(parse_size("abc").is_err());
    }
}
