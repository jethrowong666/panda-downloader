//! 视频网站下载：yt-dlp 只负责解析（标题、封面、各清晰度的真实地址），
//! 下载交给我们自己的多连接引擎（支持断点续传），最后用 ffmpeg 合并音视频。

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};

use crate::bridge::ReqCtx;

/// 已知的视频网站。复制这些网站的页面地址时按"视频"处理，而不是下载网页本身。
const VIDEO_HOSTS: &[&str] = &[
    "bilibili.com", "b23.tv", "douyin.com", "iesdouyin.com", "kuaishou.com", "ixigua.com", "toutiao.com",
    "weibo.com", "weibo.cn", "xiaohongshu.com", "xhslink.com", "youtube.com", "youtu.be", "tiktok.com",
    "twitter.com", "x.com", "vimeo.com", "dailymotion.com", "twitch.tv", "acfun.cn", "zhihu.com/zvideo",
    "v.qq.com", "youku.com", "iqiyi.com", "mgtv.com", "huya.com", "douyu.com", "instagram.com", "facebook.com",
    "reddit.com", "redd.it", "nicovideo.jp", "pornhub.com", "fb.watch", "threads.net", "threads.com", "zhihu.com/video",
    "haokan.baidu.com", "dai.ly", "bsky.app", "linkedin.com/posts", "linkedin.com/feed/update", "tumblr.com", "pinterest.com", "pin.it",
    "vk.com", "rumble.com", "kick.com", "streamable.com", "imgur.com", "9gag.com", "ted.com/talks", "bilibili.tv",
    "snapchat.com/spotlight", "soundcloud.com", "bandcamp.com", "mixcloud.com", "clips.twitch.tv", "gifshow.com",
];

pub fn is_video_page(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else { return false };
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    let full = format!("{host}{}", u.path());
    VIDEO_HOSTS.iter().any(|h| {
        if h.contains('/') {
            full.contains(h)
        } else {
            host == *h || host.ends_with(&format!(".{h}"))
        }
    })
}

/// 国内网站和它们的视频 CDN。访问这些地址时直接连接，不走系统代理：
/// 开着 VPN / Clash 全局代理时，请求会绕到海外，B 站等会当成境外访问，报"视频已删除或有地区限制"，
/// 能下载的也会被分到很慢的海外节点。
const DIRECT_HOSTS: &[&str] = &[
    "bilibili.com", "b23.tv", "bilivideo.com", "bilivideo.cn", "hdslb.com", "biliapi.net", "biliapi.com",
    "douyin.com", "iesdouyin.com", "douyinvod.com", "douyinpic.com", "douyincdn.com", "zjcdn.com", "bytecdn.cn",
    "snssdk.com", "amemv.com", "kuaishou.com", "kwimgs.com", "yximgs.com", "ixigua.com", "toutiao.com",
    "weibo.com", "weibo.cn", "sinaimg.cn", "xiaohongshu.com", "xhscdn.com", "xhslink.com", "acfun.cn",
    "qq.com", "gtimg.com", "youku.com", "iqiyi.com", "mgtv.com", "huya.com", "douyu.com", "zhihu.com", "zhimg.com",
    "haokan.baidu.com", "baidu.com", "bdstatic.com", "bcebos.com", "gifshow.com", "kwaicdn.com", "ixigua.com", "pstatp.com",
];

pub fn prefers_direct(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else { return false };
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    host.ends_with(".cn") || DIRECT_HOSTS.iter().any(|h| host == *h || host.ends_with(&format!(".{h}")))
}

/// 解析失败时把 yt-dlp 的原始输出记下来，方便排查（%APPDATA%\com.panda.downloader\video.log，只保留最近一段）。
fn log_failure(url: &str, stderr: &str) {
    let Some(dir) = dirs::config_dir().map(|d| d.join("com.panda.downloader")) else { return };
    let path = dir.join("video.log");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let keep = if old.len() > 200_000 { &old[old.len() - 100_000..] } else { &old[..] };
    let keep = keep.find('\n').map_or(keep, |i| if old.len() > 200_000 { &keep[i + 1..] } else { keep });
    let when = crate::store::now_secs();
    let tail: String = stderr.lines().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&path, format!("{keep}[{when}] {url}\n{tail}\n\n"));
}

/// 抖音网页的地址五花八门（推荐流 `?modal_id=`、精选页、搜索页、用户页、短链接、分享页），
/// yt-dlp 只认 `www.douyin.com/video/<编号>`，这里统一换算。返回 Err 表示不是视频（比如图文作品）。
fn douyin_canonical(u: &url::Url) -> Option<Result<String, String>> {
    let host = u.host_str()?.to_ascii_lowercase();
    if !(host.ends_with("douyin.com") || host.ends_with("iesdouyin.com")) {
        return None;
    }
    let id_ok = |s: &str| s.len() >= 15 && s.chars().all(|c| c.is_ascii_digit());
    if let Some((_, v)) = u.query_pairs().find(|(k, _)| k == "modal_id" || k == "vid" || k == "aweme_id") {
        if id_ok(&v) {
            return Some(Ok(format!("https://www.douyin.com/video/{v}")));
        }
    }
    let segs: Vec<&str> = u.path_segments().map(|s| s.filter(|x| !x.is_empty()).collect()).unwrap_or_default();
    for w in segs.windows(2) {
        match (w[0], w[1]) {
            ("video", id) if id_ok(id) => return Some(Ok(format!("https://www.douyin.com/video/{id}"))),
            ("note" | "slides", id) if id_ok(id) => return Some(Err("这是抖音的图文作品，不是视频".into())),
            _ => {}
        }
    }
    None
}

/// 把视频页地址整理成解析器认识的样子（必要时先跟随短链接跳转）。
pub async fn normalize_page_url(page_url: &str) -> Result<String, String> {
    let Ok(mut u) = url::Url::parse(page_url) else { return Ok(page_url.to_string()) };
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    // 抖音短链接：先跟随跳转拿到带编号的地址
    if host == "v.douyin.com" || host == "v.iesdouyin.com" {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148")
            .build()
            .map_err(|e| e.to_string())?;
        if let Ok(resp) = client.get(u.as_str()).send().await {
            u = resp.url().clone();
        }
    }
    match douyin_canonical(&u) {
        Some(r) => r,
        None => Ok(u.to_string()),
    }
}

/// 流媒体清单（扩展嗅探到的 m3u8 / mpd），交给 yt-dlp 解析下载。
pub fn is_stream_manifest(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else { return false };
    let p = u.path().to_ascii_lowercase();
    p.ends_with(".m3u8") || p.ends_with(".mpd")
}

/// 浏览器 Cookie 的临时文件，用完自动删除。
struct CookieFile(PathBuf);

impl CookieFile {
    fn new(ctx: &ReqCtx) -> Option<Self> {
        if ctx.cookies.is_empty() {
            return None;
        }
        let mut r = [0u8; 8];
        getrandom::fill(&mut r).ok()?;
        let name: String = r.iter().map(|b| format!("{b:02x}")).collect();
        let p = std::env::temp_dir().join(format!("panda-ck-{name}.txt"));
        std::fs::write(&p, ctx.netscape()).ok()?;
        Some(Self(p))
    }
}

impl Drop for CookieFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// 把浏览器上下文加到 yt-dlp 命令行上。返回的 Cookie 文件要活到命令结束。
/// 国内网站加 `--proxy ""` 直接连接（yt-dlp 默认会用 Windows 系统代理）。
fn apply_ctx(cmd: &mut tokio::process::Command, ctx: &ReqCtx, url: &str) -> Option<CookieFile> {
    if prefers_direct(url) {
        cmd.arg("--proxy").arg("");
    }
    if let Some(r) = &ctx.referer {
        cmd.arg("--referer").arg(r);
    }
    if let Some(ua) = &ctx.user_agent {
        cmd.arg("--user-agent").arg(ua);
    }
    let f = CookieFile::new(ctx);
    if let Some(f) = &f {
        cmd.arg("--cookies").arg(&f.0);
    }
    f
}

/// 找外部工具：程序目录下的 tools\，开发时的项目 tools\，再找 PATH。
pub fn tool(name: &str) -> Option<PathBuf> {
    let exe = format!("{name}.exe");
    if let Ok(me) = std::env::current_exe() {
        let dir = me.parent().map(Path::to_path_buf).unwrap_or_default();
        for c in [dir.join("tools").join(&exe), dir.join(&exe), dir.join("..").join("..").join("tools").join(&exe)] {
            if c.exists() {
                return Some(c);
            }
        }
    }
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(&exe)).find(|c| c.exists()))
}

fn hidden(cmd: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let _ = cmd;
}

/// 一路流（视频、音频或音视频合一）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stream {
    pub format_id: String,
    pub url: String,
    pub ext: String,
    /// 是否是普通 http(s) 文件（能用我们的引擎下）；m3u8/分片流交给 yt-dlp。
    pub direct: bool,
    pub size: Option<u64>,
    pub headers: Vec<(String, String)>,
}

/// 一个可选的清晰度。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    /// 例如 "1080"、"720"、"audio"。
    pub key: String,
    pub label: String,
    pub detail: String,
    pub size: Option<u64>,
    pub video: Option<Stream>,
    pub audio: Option<Stream>,
    /// 输出格式：mp4 / mkv / m4a……
    pub container: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoInfo {
    pub page_url: String,
    pub title: String,
    pub thumbnail: Option<String>,
    pub duration: Option<f64>,
    pub uploader: Option<String>,
    pub site: String,
    pub choices: Vec<Choice>,
    /// 解析时用了浏览器 Cookie（下载分片流时要用同样的方式，否则格式编号对不上）。
    #[serde(default)]
    pub use_cookies: bool,
}

#[derive(Deserialize)]
struct RawInfo {
    title: Option<String>,
    thumbnail: Option<String>,
    duration: Option<f64>,
    uploader: Option<String>,
    extractor_key: Option<String>,
    webpage_url: Option<String>,
    #[serde(default)]
    formats: Vec<RawFormat>,
    // 单一格式的网站，信息直接在顶层
    url: Option<String>,
    ext: Option<String>,
    format_id: Option<String>,
    #[serde(default)]
    http_headers: std::collections::HashMap<String, String>,
}

#[derive(Deserialize, Clone)]
struct RawFormat {
    format_id: String,
    url: Option<String>,
    ext: Option<String>,
    height: Option<u32>,
    vcodec: Option<String>,
    acodec: Option<String>,
    filesize: Option<u64>,
    filesize_approx: Option<u64>,
    tbr: Option<f64>,
    abr: Option<f64>,
    protocol: Option<String>,
    #[serde(default)]
    http_headers: std::collections::HashMap<String, String>,
}

impl RawFormat {
    fn has_video(&self) -> bool {
        self.vcodec.as_deref().is_some_and(|v| v != "none") || (self.vcodec.is_none() && self.height.is_some())
    }
    fn has_audio(&self) -> bool {
        self.acodec.as_deref().is_some_and(|a| a != "none")
    }
    fn size(&self) -> Option<u64> {
        self.filesize.or(self.filesize_approx)
    }
    /// 兼容性：H.264 在任何播放器都能放；H.265 次之；AV1 老设备放不了。
    fn codec_rank(&self) -> u8 {
        let v = self.vcodec.as_deref().unwrap_or("");
        if v.starts_with("avc") || v.starts_with("h264") {
            3
        } else if v.starts_with("hev") || v.starts_with("hvc") || v.starts_with("h265") {
            2
        } else if v.starts_with("vp9") || v.starts_with("vp09") {
            1
        } else {
            0
        }
    }
    fn stream(&self) -> Option<Stream> {
        let url = self.url.clone()?;
        let proto = self.protocol.as_deref().unwrap_or("https");
        Some(Stream {
            format_id: self.format_id.clone(),
            url,
            ext: self.ext.clone().unwrap_or_else(|| "mp4".into()),
            direct: matches!(proto, "https" | "http"),
            size: self.size(),
            headers: headers_vec(&self.http_headers),
        })
    }
}

fn headers_vec(h: &std::collections::HashMap<String, String>) -> Vec<(String, String)> {
    // Accept 系列交给下载器自己处理，只带对防盗链有用的头。
    h.iter()
        .filter(|(k, _)| matches!(k.to_ascii_lowercase().as_str(), "referer" | "user-agent" | "cookie" | "origin"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn quality_label(h: u32) -> String {
    match h {
        h if h >= 2160 => "4K 超清".into(),
        h if h >= 1440 => "2K".into(),
        h if h >= 1080 => "1080P 高清".into(),
        h if h >= 720 => "720P 清晰".into(),
        h if h >= 480 => "480P 标清".into(),
        h => format!("{h}P 流畅"),
    }
}

/// YouTube：带着登录 Cookie 解析时，yt-dlp 会换成必须运行 JavaScript 的网页版客户端，
/// 大部分视频反而失败；不带 Cookie 用手机客户端，能拿到最高画质。所以先不带，失败了再带上重试。
fn is_youtube(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|h| h == "youtu.be" || h == "youtube.com" || h.ends_with(".youtube.com"))
}

/// 有 JavaScript 运行组件（tools\deno.exe）就告诉 yt-dlp，YouTube 登录状态下解析要用。
fn add_js_runtime(cmd: &mut tokio::process::Command) {
    if let Some(deno) = tool("deno") {
        cmd.arg("--js-runtimes").arg(format!("deno:{}", deno.display()));
    }
}

/// 用 yt-dlp 解析视频页。
pub async fn resolve(page_url: &str, ctx: &ReqCtx) -> Result<VideoInfo, String> {
    let normalized = normalize_page_url(page_url).await?;
    let page_url = normalized.as_str();
    if is_youtube(page_url) && !ctx.cookies.is_empty() {
        let anon = ReqCtx { cookies: Vec::new(), ..ctx.clone() };
        match resolve_once(page_url, &anon).await {
            Ok(info) => return Ok(info),
            Err(first) => {
                return match resolve_once(page_url, ctx).await {
                    Ok(mut info) => {
                        info.use_cookies = true;
                        Ok(info)
                    }
                    // 两次都失败：带 Cookie 那次的原因通常更有用（比如会员视频）
                    Err(second) => Err(if second.starts_with("YouTube 没有返回") { first } else { second }),
                };
            }
        }
    }
    let mut info = resolve_once(page_url, ctx).await?;
    info.use_cookies = !ctx.cookies.is_empty();
    Ok(info)
}

async fn resolve_once(page_url: &str, ctx: &ReqCtx) -> Result<VideoInfo, String> {
    let ytdlp = tool("yt-dlp").ok_or("缺少视频解析组件 yt-dlp")?;
    let mut cmd = tokio::process::Command::new(ytdlp);
    let _cookies = apply_ctx(&mut cmd, ctx, page_url);
    add_js_runtime(&mut cmd);
    cmd.args(["-J", "--no-playlist", "--no-warnings", "--encoding", "utf-8", "--socket-timeout", "20", page_url])
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    hidden(&mut cmd);
    let out = tokio::time::timeout(std::time::Duration::from_secs(120), cmd.output())
        .await
        .map_err(|_| "解析超时，网络可能不通".to_string())?
        .map_err(|e| format!("无法启动 yt-dlp：{e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        log_failure(page_url, &stderr);
        return Err(friendly_error(&stderr));
    }
    let raw: RawInfo = serde_json::from_slice(&out.stdout).map_err(|e| format!("解析结果无法识别：{e}"))?;
    build(raw, page_url)
}
/// 给用户看的原因 + 一行原始信息（"详情："开头），方便截图排查。
fn friendly_error(stderr: &str) -> String {
    let line = stderr.lines().rev().find(|l| l.contains("ERROR")).unwrap_or(stderr.trim()).to_string();
    let raw: String = line.trim_start_matches("ERROR:").trim().chars().take(200).collect();
    let msg = friendly_text(&line);
    if raw.is_empty() || msg.starts_with("解析失败") {
        msg
    } else {
        format!("{msg}\n详情：{raw}")
    }
}

fn friendly_text(line: &str) -> String {
    let low = line.to_ascii_lowercase();
    // 抖音、快手、西瓜：接口要签名，只能由浏览器扩展从网页里拿视频地址
    let browser_site = [
        ("douyin", "抖音视频"),
        ("kuaishou", "快手视频"),
        ("ixigua", "西瓜视频"),
        ("xiaohongshu", "小红书视频"),
        ("weibo", "微博视频"),
        ("tiktok", "TikTok 视频"),
        ("instagram", "Instagram 视频"),
        ("twitter", "X（推特）视频"),
        ("x.com", "X（推特）视频"),
        ("reddit", "Reddit 视频"),
    ]
    .into_iter()
    .find(|(k, _)| low.contains(k));
    let needs_browser = low.contains("unsupported url")
        || low.contains("cookies")
        || low.contains("login")
        || low.contains("log in")
        || low.contains("rate-limit")
        || low.contains("rate limit")
        || low.contains("authentication")
        || low.contains("ip address is blocked");
    if let Some((_, name)) = browser_site.filter(|_| needs_browser) {
        format!("{name}要通过浏览器下载：在浏览器里打开这个视频（装好熊猫下载器扩展后先刷新页面），点视频右上角的「熊猫下载」")
    } else if low.contains("unsupported url") {
        "这个网址里没有找到视频".into()
    } else if low.contains("unable to connect to proxy") || low.contains("proxyerror") {
        "代理连不上：系统设置了代理，但代理软件没有运行。打开代理软件，或者在 Windows 设置里关掉代理".into()
    } else if low.contains("not a bot") {
        "YouTube 要求验证不是机器人。请在浏览器里打开这个视频，用视频上的「熊猫下载」按钮下载".into()
    } else if low.contains("requested format is not available") || low.contains("only images are available") {
        "YouTube 没有返回能下载的画质。请把熊猫下载器更新到最新版；如果还不行，换个网络或节点再试".into()
    } else if low.contains("geo-restricted") || low.contains("geo restricted") || low.contains("not available in your country") {
        "这个视频有地区限制，当前网络所在地区看不了。海外视频要开代理；国内视频请关掉代理的「全局模式」".into()
    } else if low.contains("deleted")
        || low.contains("not available")
        || low.contains("unavailable")
        // 只认真正的 HTTP 404；视频编号里碰巧有"404"三个数字的不算（抖音 7710865501240429864 就被误判过）
        || low.contains("http error 404")
        || low.contains("404: not found")
        || low.contains("404 not found")
    {
        "视频不存在或已被删除".into()
    } else if low.contains("fresh cookies") && low.contains("douyin") {
        "抖音视频要通过浏览器下载：在浏览器里打开这个视频（装好熊猫下载器扩展后先刷新页面），点视频右上角的「熊猫下载」".into()
    } else if low.contains("fresh cookies") {
        "这个网站要通过浏览器下载：在浏览器里打开视频（装好熊猫下载器扩展后先刷新页面），点视频右上角的「熊猫下载」".into()
    } else if low.contains("cookies") || low.contains("login") || low.contains("sign in") {
        "这个视频需要登录才能下载（装好浏览器扩展后会自动使用浏览器的登录状态）".into()
    } else if low.contains("drm") {
        "这个视频有版权加密（DRM），无法下载".into()
    } else if low.contains("timed out") || low.contains("unable to download webpage") || low.contains("connection") {
        "连不上这个视频网站，检查一下网络".into()
    } else {
        let msg = line.trim_start_matches("ERROR:").trim();
        format!("解析失败：{}", msg.chars().take(160).collect::<String>())
    }
}

fn build(raw: RawInfo, page_url: &str) -> Result<VideoInfo, String> {
    let mut formats = raw.formats.clone();
    if formats.is_empty() {
        // 只有一个格式的网站
        if let Some(url) = &raw.url {
            formats.push(RawFormat {
                format_id: raw.format_id.clone().unwrap_or_else(|| "0".into()),
                url: Some(url.clone()),
                ext: raw.ext.clone(),
                height: None,
                vcodec: None,
                acodec: Some("unknown".into()),
                filesize: None,
                filesize_approx: None,
                tbr: None,
                abr: None,
                protocol: Some("https".into()),
                http_headers: raw.http_headers.clone(),
            });
        }
    }
    let usable: Vec<&RawFormat> = formats.iter().filter(|f| f.url.is_some()).collect();
    let best_audio = usable
        .iter()
        .filter(|f| f.has_audio() && !f.has_video())
        .max_by(|a, b| {
            let pa = a.protocol.as_deref().is_some_and(|p| p.starts_with("http"));
            let pb = b.protocol.as_deref().is_some_and(|p| p.starts_with("http"));
            // 优先 m4a（合并成 mp4 最兼容），再看码率
            (pa, a.ext.as_deref() == Some("m4a"), a.abr.or(a.tbr).unwrap_or(0.0) as u64)
                .cmp(&(pb, b.ext.as_deref() == Some("m4a"), b.abr.or(b.tbr).unwrap_or(0.0) as u64))
        })
        .copied();

    let mut heights: Vec<u32> = usable.iter().filter(|f| f.has_video()).filter_map(|f| f.height).collect();
    heights.sort_unstable_by(|a, b| b.cmp(a));
    heights.dedup();

    let mut choices = Vec::new();
    for h in &heights {
        // 同一清晰度里：优先能直接下载的、兼容性好的编码，再看码率
        let pick = usable
            .iter()
            .filter(|f| f.has_video() && f.height == Some(*h))
            .max_by(|a, b| {
                let da = a.protocol.as_deref().is_some_and(|p| p.starts_with("http"));
                let db = b.protocol.as_deref().is_some_and(|p| p.starts_with("http"));
                (da, a.has_audio() as u8 + a.codec_rank(), a.tbr.unwrap_or(0.0) as u64)
                    .cmp(&(db, b.has_audio() as u8 + b.codec_rank(), b.tbr.unwrap_or(0.0) as u64))
            })
            .copied();
        let Some(v) = pick else { continue };
        let audio = if v.has_audio() { None } else { best_audio };
        let container = match (v.ext.as_deref(), audio.and_then(|a| a.ext.as_deref())) {
            (Some("mp4"), Some("m4a") | None) => "mp4",
            (Some("webm"), Some("webm") | None) => "webm",
            (Some(e), None) => e,
            _ => "mkv",
        };
        let size = match (v.size(), audio.and_then(RawFormat::size)) {
            (Some(a), Some(b)) => Some(a + b),
            (Some(a), None) if audio.is_none() => Some(a),
            _ => v.size(),
        };
        let codec = match v.codec_rank() {
            3 => "H.264",
            2 => "H.265",
            1 => "VP9",
            _ => "AV1",
        };
        choices.push(Choice {
            key: h.to_string(),
            label: quality_label(*h),
            detail: format!("{} · {}", container.to_uppercase(), codec),
            size,
            video: v.stream(),
            audio: audio.and_then(RawFormat::stream),
            container: container.into(),
        });
    }
    // 没有标注清晰度的网站（只给一个文件）
    if choices.is_empty() {
        if let Some(v) = usable.iter().filter(|f| f.has_video() || f.has_audio()).max_by_key(|f| f.tbr.unwrap_or(0.0) as u64) {
            choices.push(Choice {
                key: "best".into(),
                label: "最佳画质".into(),
                detail: v.ext.clone().unwrap_or_default().to_uppercase(),
                size: v.size(),
                video: v.stream(),
                audio: None,
                container: v.ext.clone().unwrap_or_else(|| "mp4".into()),
            });
        }
    }
    if let Some(a) = best_audio {
        choices.push(Choice {
            key: "audio".into(),
            label: "仅音频".into(),
            detail: a.ext.clone().unwrap_or_default().to_uppercase(),
            size: a.size(),
            video: None,
            audio: a.stream(),
            container: a.ext.clone().unwrap_or_else(|| "m4a".into()),
        });
    }
    if choices.is_empty() {
        return Err("这个网址不是单个视频的页面（可能是首页、频道或列表）。请打开要下载的那个视频，再复制它的地址".into());
    }
    Ok(VideoInfo {
        page_url: raw.webpage_url.unwrap_or_else(|| page_url.to_string()),
        title: raw.title.unwrap_or_else(|| "视频".into()),
        thumbnail: raw.thumbnail.filter(|t| t.starts_with("http")),
        duration: raw.duration,
        uploader: raw.uploader,
        site: site_name(raw.extractor_key.as_deref().unwrap_or("")),
        choices,
        use_cookies: false,
    })
}

fn site_name(key: &str) -> String {
    let k = key.to_ascii_lowercase();
    let name = if k.starts_with("bili") {
        "哔哩哔哩"
    } else if k.starts_with("douyin") {
        "抖音"
    } else if k.starts_with("kuaishou") {
        "快手"
    } else if k.starts_with("ixigua") {
        "西瓜视频"
    } else if k.starts_with("youtube") {
        "YouTube"
    } else if k.starts_with("weibo") {
        "微博"
    } else if k.starts_with("xiaohongshu") {
        "小红书"
    } else if k.starts_with("tiktok") {
        "TikTok"
    } else {
        return key.to_string();
    };
    name.into()
}

/// B站的视频文件在多个 CDN 节点上都有，签名不绑定节点。
/// 开了代理的用户常被分到海外节点（很慢），所以下载前测一下速，挑最快的。
const BILI_MIRRORS: &[&str] = &[
    "upos-sz-mirror08c.bilivideo.com",
    "upos-sz-mirrorali.bilivideo.com",
    "upos-sz-mirrorhw.bilivideo.com",
    "upos-sz-mirrorcos.bilivideo.com",
];

pub async fn pick_fastest_mirror(mut s: Stream) -> Stream {
    let Ok(url) = url::Url::parse(&s.url) else { return s };
    let host = url.host_str().unwrap_or("").to_string();
    let is_bili = (host.contains("bilivideo") || host.contains("akamaized")) && url.path().contains("/upgcxcode/");
    if !is_bili || !s.direct {
        return s;
    }
    let mut candidates: Vec<String> = vec![s.url.clone()];
    for m in BILI_MIRRORS {
        if *m != host {
            let mut u = url.clone();
            if u.set_host(Some(m)).is_ok() {
                candidates.push(u.to_string());
            }
        }
    }
    let Ok(client) = reqwest::Client::builder().no_proxy().timeout(std::time::Duration::from_secs(4)).build() else { return s };
    let headers = s.headers.clone();
    let probes = candidates.into_iter().map(|c| {
        let client = client.clone();
        let headers = headers.clone();
        async move {
            let start = std::time::Instant::now();
            let mut req = client.get(&c).header("Range", "bytes=0-524287");
            for (k, v) in &headers {
                req = req.header(k.as_str(), v.as_str());
            }
            let resp = req.send().await.ok()?;
            if resp.status().as_u16() != 206 {
                return None;
            }
            let n = resp.bytes().await.ok()?.len();
            (n > 0).then(|| (c, n as f64 / start.elapsed().as_secs_f64().max(0.001)))
        }
    });
    let results = futures_util::future::join_all(probes).await;
    if let Some((best, _)) = results.into_iter().flatten().max_by(|a, b| a.1.total_cmp(&b.1)) {
        s.url = best;
    }
    s
}

/// 用 ffmpeg 把视频和音频合成一个文件（不重新编码，秒级完成）。
pub async fn merge(video: &Path, audio: Option<&Path>, out: &Path, cancel: &tokio_util::sync::CancellationToken) -> Result<(), String> {
    let ffmpeg = tool("ffmpeg").ok_or("缺少合并组件 ffmpeg")?;
    let mut cmd = tokio::process::Command::new(ffmpeg);
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"]).arg(video);
    if let Some(a) = audio {
        cmd.arg("-i").arg(a).args(["-map", "0:v:0", "-map", "1:a:0"]);
    }
    cmd.args(["-c", "copy"]);
    if out.extension().is_some_and(|e| e == "mp4" || e == "m4a") {
        // 把索引放到文件开头，边下边播、网页上传都更友好
        cmd.args(["-movflags", "+faststart"]);
    }
    cmd.arg(out).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
    hidden(&mut cmd);
    let child = cmd.spawn().map_err(|e| format!("无法启动 ffmpeg：{e}"))?;
    tokio::select! {
        _ = cancel.cancelled() => Err("已暂停".into()),
        r = child.wait_with_output() => {
            let o = r.map_err(|e| e.to_string())?;
            if o.status.success() { Ok(()) } else {
                Err(format!("合并音视频失败：{}", String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("")))
            }
        }
    }
}

/// 分片流（m3u8 等）我们的引擎下不了，交给 yt-dlp 直接下载这一路。
pub async fn ytdlp_download(
    page_url: &str,
    format_id: &str,
    out: &Path,
    cancel: &tokio_util::sync::CancellationToken,
    ctx: &ReqCtx,
) -> Result<(), String> {
    let ytdlp = tool("yt-dlp").ok_or("缺少视频解析组件 yt-dlp")?;
    let mut cmd = tokio::process::Command::new(ytdlp);
    let _cookies = apply_ctx(&mut cmd, ctx, page_url);
    add_js_runtime(&mut cmd);
    cmd.args(["--no-playlist", "--no-warnings", "--encoding", "utf-8", "-N", "8", "--continue", "-f", format_id, "-o"])
        .arg(out)
        .arg(page_url)
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(ff) = tool("ffmpeg") {
        cmd.arg("--ffmpeg-location").arg(ff);
    }
    hidden(&mut cmd);
    let child = cmd.spawn().map_err(|e| format!("无法启动 yt-dlp：{e}"))?;
    tokio::select! {
        _ = cancel.cancelled() => Err("已暂停".into()),
        r = child.wait_with_output() => {
            let o = r.map_err(|e| e.to_string())?;
            if o.status.success() { Ok(()) } else {
                let stderr = String::from_utf8_lossy(&o.stderr);
                log_failure(page_url, &stderr);
                Err(friendly_error(&stderr))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_hosts() {
        assert!(is_video_page("https://www.bilibili.com/video/BV1xx411c7mD"));
        assert!(is_video_page("https://b23.tv/abc"));
        assert!(is_video_page("https://v.douyin.com/iRNBho6u/"));
        assert!(is_video_page("https://youtu.be/dQw4w9WgXcQ"));
        assert!(is_video_page("https://www.zhihu.com/zvideo/123"));
        assert!(is_video_page("https://www.threads.net/@user/post/C1abc"));
        assert!(is_video_page("https://haokan.baidu.com/v?vid=123"));
        assert!(is_video_page("https://vimeo.com/76979871"));
        assert!(is_video_page("https://www.linkedin.com/posts/abc-123"));
        assert!(!is_video_page("https://www.linkedin.com/in/someone"));
        assert!(prefers_direct("https://vd3.bdstatic.com/mda-abc/1080p.mp4"));
        assert!(!is_video_page("https://www.zhihu.com/question/1"));
        assert!(!is_video_page("https://example.com/a.mp4"));
        assert!(!is_video_page("https://notbilibili.com/x"));
    }

    #[test]
    fn direct_routing() {
        assert!(prefers_direct("https://www.bilibili.com/video/BV1xx411c7mD"));
        assert!(prefers_direct("https://upos-sz-mirror08c.bilivideo.com/upgcxcode/x.m4s"));
        assert!(prefers_direct("https://v26-web.douyinvod.com/abc/video/tos/cn/x/"));
        assert!(prefers_direct("https://example.gov.cn/a.pdf"));
        assert!(!prefers_direct("https://www.youtube.com/watch?v=jNQXAC9IVRw"));
        assert!(!prefers_direct("https://rr1---sn-x.googlevideo.com/videoplayback"));
        assert!(!prefers_direct("https://notbilibili.com/x"));
    }

    #[test]
    fn errors_keep_raw_detail() {
        let e = friendly_error("ERROR: [BiliBili] 1GJ: This video may be deleted or geo-restricted. You might want to try a VPN");
        assert!(e.starts_with("这个视频有地区限制"));
        assert!(e.contains("\n详情：[BiliBili] 1GJ: This video may be deleted or geo-restricted"));
        assert!(friendly_error("ERROR: [youtube] x: Video unavailable").starts_with("视频不存在或已被删除"));
        assert!(friendly_error("ERROR: Unable to connect to proxy").starts_with("代理连不上"));
        // 用户日志里的原文
        let dy = friendly_error("ERROR: [Douyin] 7710865501240429864: Fresh cookies (not necessarily logged in) are needed; please report this issue on  https://github.com/yt-dlp/yt-dlp/issues?q= , filling out the appropriate issue template. Confirm you are on the latest version using  yt-dlp -U");
        assert!(dy.starts_with("抖音视频要通过浏览器下载"), "{dy}");
        assert!(friendly_error("ERROR: Unsupported URL: https://www.kuaishou.com/short-video/3xabc").starts_with("快手视频要通过浏览器下载"));
        assert!(friendly_error("ERROR: [Ixigua] 7012: Cookies (not necessarily logged in) are needed").starts_with("西瓜视频要通过浏览器下载"));
        assert!(friendly_error("ERROR: [Instagram] C1a: Requested content is not available, rate-limit reached or login required").starts_with("Instagram 视频要通过浏览器下载"));
        assert!(friendly_error("ERROR: [XiaoHongShu] 65ab: Unable to extract; please pass --cookies").starts_with("小红书视频要通过浏览器下载"));
        assert!(friendly_error("ERROR: [x] 1: Unable to download JSON metadata: HTTP Error 404: Not Found").starts_with("视频不存在"));
        assert!(!friendly_error("ERROR: [BiliBili] BV1404xx: Some new problem").starts_with("视频不存在"));
    }

    #[test]
    fn douyin_urls() {
        let c = |s: &str| douyin_canonical(&url::Url::parse(s).unwrap());
        let want = Some(Ok("https://www.douyin.com/video/7301234567890123456".to_string()));
        assert_eq!(c("https://www.douyin.com/?recommend=1&modal_id=7301234567890123456"), want);
        assert_eq!(c("https://www.douyin.com/jingxuan?modal_id=7301234567890123456"), want);
        assert_eq!(c("https://www.douyin.com/user/MS4wLjABAAAA?modal_id=7301234567890123456&vid=1"), want);
        assert_eq!(c("https://www.douyin.com/video/7301234567890123456?previous_page=app"), want);
        assert_eq!(c("https://www.iesdouyin.com/share/video/7301234567890123456/?region=CN"), want);
        assert!(matches!(c("https://www.douyin.com/note/7301234567890123456"), Some(Err(_))));
        assert_eq!(c("https://www.douyin.com/?recommend=1"), None);
        assert_eq!(c("https://www.bilibili.com/video/BV1xx411c7mD"), None);
    }

    #[test]
    fn builds_choices_from_dash() {
        let json = r#"{"title":"测试","extractor_key":"BiliBili","webpage_url":"https://www.bilibili.com/video/BV1","duration":10.0,
          "formats":[
            {"format_id":"a1","url":"https://x/a1.m4s","ext":"m4a","vcodec":"none","acodec":"mp4a.40.2","filesize":100,"abr":64,"protocol":"https","http_headers":{"Referer":"https://www.bilibili.com/","Accept":"*/*"}},
            {"format_id":"a2","url":"https://x/a2.m4s","ext":"m4a","vcodec":"none","acodec":"mp4a.40.2","filesize":200,"abr":128,"protocol":"https"},
            {"format_id":"v1","url":"https://x/v1.m4s","ext":"mp4","height":480,"vcodec":"av01.0","acodec":"none","filesize":1000,"tbr":500,"protocol":"https"},
            {"format_id":"v2","url":"https://x/v2.m4s","ext":"mp4","height":480,"vcodec":"avc1.64001E","acodec":"none","filesize":2000,"tbr":400,"protocol":"https"},
            {"format_id":"v3","url":"https://x/v3.m4s","ext":"mp4","height":1080,"vcodec":"avc1.640032","acodec":"none","filesize":9000,"tbr":2000,"protocol":"https"}
          ]}"#;
        let raw: RawInfo = serde_json::from_str(json).unwrap();
        let info = build(raw, "https://www.bilibili.com/video/BV1").unwrap();
        assert_eq!(info.site, "哔哩哔哩");
        let keys: Vec<&str> = info.choices.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(keys, vec!["1080", "480", "audio"]);
        // 480P 选 H.264 而不是 AV1
        assert_eq!(info.choices[1].video.as_ref().unwrap().format_id, "v2");
        // 音频选码率更高的，大小相加
        assert_eq!(info.choices[0].audio.as_ref().unwrap().format_id, "a2");
        assert_eq!(info.choices[0].size, Some(9200));
        assert_eq!(info.choices[0].container, "mp4");
    }
}
