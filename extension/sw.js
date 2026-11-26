// 熊猫下载器 · 浏览器扩展后台
// 1. 接管下载：浏览器开始下载时取消它，连同 Cookie、来源页一起交给熊猫下载器；失败就交还浏览器。
// 2. 视频嗅探：记录每个标签页里出现的视频流（m3u8 / mpd / mp4…），图标上显示数量。
// 3. 右键菜单：下载链接、下载视频、下载本页所有文件链接。

const HOST = "com.panda.downloader";
const MB = 1024 * 1024;

// 视频网站：交给下载器用 yt-dlp 解析（和下载器里的列表保持一致）
const VIDEO_SITES = ["bilibili.com", "b23.tv", "douyin.com", "iesdouyin.com", "kuaishou.com", "ixigua.com", "toutiao.com",
  "weibo.com", "xiaohongshu.com", "youtube.com", "youtu.be", "tiktok.com", "twitter.com", "x.com", "vimeo.com",
  "acfun.cn", "v.qq.com", "youku.com", "iqiyi.com", "mgtv.com", "huya.com", "douyu.com", "instagram.com", "facebook.com", "nicovideo.jp",
  "reddit.com", "redd.it", "xhslink.com", "weibo.cn", "gifshow.com", "twitch.tv", "dailymotion.com",
  "fb.watch", "threads.net", "threads.com", "zhihu.com", "haokan.baidu.com", "vimeo.com", "dai.ly", "bsky.app", "linkedin.com",
  "tumblr.com", "pinterest.com", "pin.it", "vk.com", "rumble.com", "kick.com", "streamable.com", "imgur.com", "9gag.com",
  "ted.com", "bilibili.tv", "snapchat.com", "soundcloud.com", "bandcamp.com", "mixcloud.com"];
const FILE_EXT = /\.(zip|rar|7z|tar|gz|tgz|bz2|xz|zst|iso|exe|msi|msix|apk|dmg|pkg|deb|rpm|appimage|mp4|mkv|avi|mov|wmv|flv|webm|m4v|ts|mp3|flac|wav|aac|m4a|ogg|opus|pdf|docx?|xlsx?|pptx?|epub|mobi|torrent|bin|img|jar|whl)(\?|#|$)/i;

const settings = { takeover: true };
chrome.storage.local.get({ takeover: true }).then((s) => Object.assign(settings, s));
chrome.storage.onChanged.addListener((c) => { if (c.takeover) settings.takeover = c.takeover.newValue; });

// 商店版屏蔽的网站。普通版是空的；打包 Chrome 商店版时由 scripts/pack_extension.py 填上 YouTube
// （Chrome 应用商店不允许扩展下载 YouTube 视频）。
const STORE_BLOCK = [];
const STORE_BLOCK_MSG = "应 Chrome 应用商店的规定，这个版本的扩展不支持 YouTube";
function blocked(url) {
  if (!STORE_BLOCK.length || !url) return false;
  try {
    const h = new URL(url).hostname.toLowerCase();
    return STORE_BLOCK.some((s) => h === s || h.endsWith("." + s));
  } catch { return false; }
}

function isVideoSite(url) {
  if (blocked(url)) return false;
  try {
    const h = new URL(url).hostname.toLowerCase();
    return VIDEO_SITES.some((s) => h === s || h.endsWith("." + s));
  } catch { return false; }
}

function native(msg) {
  return new Promise((resolve) => {
    try {
      chrome.runtime.sendNativeMessage(HOST, msg, (r) => {
        if (chrome.runtime.lastError) resolve({ ok: false, error: chrome.runtime.lastError.message });
        else resolve(r || { ok: false, error: "没有响应" });
      });
    } catch (e) { resolve({ ok: false, error: String(e) }); }
  });
}

// 扩展刚装上、刚更新、浏览器刚启动时向熊猫下载器报到（软件里的安装向导靠它显示"已连接"）
chrome.runtime.onInstalled.addListener(() => { native({ cmd: "ping" }); });
chrome.runtime.onStartup.addListener(() => { native({ cmd: "ping" }); });

// 取这些网址能用到的 Cookie（下载需要登录的文件、抖音等网站解析都靠它）
async function cookiesFor(urls) {
  const seen = new Set(), out = [];
  for (const url of urls) {
    if (!url || !/^https?:/.test(url)) continue;
    let list = [];
    try { list = await chrome.cookies.getAll({ url }); } catch {}
    for (const c of list) {
      const k = `${c.domain}|${c.path}|${c.name}`;
      if (seen.has(k)) continue;
      seen.add(k);
      out.push({ domain: c.domain, name: c.name, value: c.value, path: c.path, secure: c.secure, expirationDate: c.expirationDate });
    }
  }
  return out;
}

async function sendToApp(items, referer) {
  const urls = [...items.map((i) => i.url), referer];
  // 视频页：顺带把整个站点的 Cookie 带上（抖音的 s_v_web_id 在主域名上）
  for (const i of items) if (i.video) { try { urls.push(new URL(i.url).origin + "/"); } catch {} }
  const cookies = await cookiesFor(urls);
  return native({ cmd: "capture", payload: { items, referer: referer || null, user_agent: navigator.userAgent, cookies } });
}

// ───────── 1. 接管下载 ─────────
let altUntil = 0;
const giveBack = new Set();

function worthTaking(item, url) {
  const mime = (item.mime || "").toLowerCase();
  if (FILE_EXT.test(url)) return true;
  if (item.totalBytes > 0 && item.totalBytes < 256 * 1024) return false; // 很小的文件浏览器自己下就行
  if (/^(text\/|image\/)/.test(mime) && !(item.totalBytes > MB)) return false;
  return true;
}

function basename(p) {
  if (!p) return null;
  const n = p.split(/[\\/]/).pop();
  return n || null;
}

chrome.downloads.onCreated.addListener(async (item) => {
  const url = item.finalUrl || item.url;
  if (!settings.takeover || Date.now() < altUntil) return;    // 关掉了，或者按住 Alt 点的
  if (item.byExtensionId) return;                              // 扩展自己发起的（包括我们交还的）
  if (!/^https?:/i.test(url)) return;                          // blob:、data: 只有浏览器能下
  if (giveBack.has(url)) { giveBack.delete(url); return; }
  if (!worthTaking(item, url)) return;

  // 先暂停，等下载器确认接手了再取消，避免"下载器没反应，浏览器也没下"
  try { await chrome.downloads.pause(item.id); } catch {}
  const r = await sendToApp([{ url, filename: basename(item.filename) }], item.referrer);
  if (r && r.ok) {
    try { chrome.downloads.setUiOptions?.({ enabled: false }); } catch {}
    try { await chrome.downloads.cancel(item.id); await chrome.downloads.erase({ id: item.id }); } catch {}
    setTimeout(() => { try { chrome.downloads.setUiOptions?.({ enabled: true }); } catch {} }, 1200);
  } else {
    // 下载器不可用：让浏览器继续下载
    try { await chrome.downloads.resume(item.id); } catch { giveBack.add(url); chrome.downloads.download({ url }); }
  }
});

// ───────── 2. 视频嗅探 ─────────
const media = new Map(); // tabId -> Map(key -> {url, kind, size, mime, at})

function header(headers, name) {
  const h = (headers || []).find((x) => x.name.toLowerCase() === name);
  return h ? h.value : "";
}

function classify(url, ct, len, type) {
  const u = url.toLowerCase().split(/[?#]/)[0];
  ct = (ct || "").toLowerCase();
  if (u.endsWith(".m3u8") || ct.includes("mpegurl")) return "HLS";
  if (u.endsWith(".mpd") || ct.includes("dash+xml")) return "DASH";
  if (/\.(ts|m4s|aac)$/.test(u) || /\/seg|\/frag|chunk-/.test(u)) return null;         // 分片，不单独列
  if (ct.startsWith("video/") || /\.(mp4|webm|flv|mov|m4v|mkv)$/.test(u)) return len > 512 * 1024 || (len === 0 && type === "media") ? "视频" : null;
  if (ct.startsWith("audio/") || /\.(mp3|m4a|flac|ogg|opus)$/.test(u)) return len > 256 * 1024 ? "音频" : null;
  return null;
}

chrome.webRequest.onResponseStarted.addListener((d) => {
  if (d.tabId < 0 || d.statusCode >= 400 || /^(blob|data):/.test(d.url)) return;
  if (blocked(d.url) || blocked(d.initiator)) return;
  const len = +header(d.responseHeaders, "content-length") || 0;
  const range = header(d.responseHeaders, "content-range");
  const total = range ? +range.split("/")[1] || len : len;
  const kind = classify(d.url, header(d.responseHeaders, "content-type"), total, d.type);
  if (!kind) return;
  if (!media.has(d.tabId)) media.set(d.tabId, new Map());
  const key = d.url.split(/[?#]/)[0];
  const m = media.get(d.tabId);
  const old = m.get(key);
  m.set(key, { url: d.url, kind, size: Math.max(total, old?.size || 0), at: Date.now() });
  updateBadge(d.tabId);
}, { urls: ["http://*/*", "https://*/*"], types: ["media", "xmlhttprequest", "other", "object"] }, ["responseHeaders"]);

chrome.tabs.onUpdated.addListener((tabId, info, tab) => {
  if (info.url) { media.delete(tabId); updateBadge(tabId, tab.url); }
});
chrome.tabs.onRemoved.addListener((tabId) => media.delete(tabId));

async function updateBadge(tabId, url) {
  if (!url) { try { url = (await chrome.tabs.get(tabId)).url; } catch { return; } }
  const n = (media.get(tabId)?.size || 0);
  const text = isVideoSite(url) ? (n ? String(n + 1) : "1") : n ? String(n) : "";
  chrome.action.setBadgeText({ tabId, text });
  chrome.action.setBadgeBackgroundColor({ tabId, color: "#12b886" });
}

function mediaList(tabId) {
  return [...(media.get(tabId)?.values() || [])].sort((a, b) => (a.kind === "HLS" || a.kind === "DASH" ? -1 : 0) - (b.kind === "HLS" || b.kind === "DASH" ? -1 : 0) || b.size - a.size);
}

// 问页面"现在是哪个视频"：首页、推荐流里的预览播放，地址栏不是视频自己的地址
async function askPage(tabId, fromContextMenu) {
  try {
    return (await chrome.tabs.sendMessage(tabId, { type: "findPage", fromContextMenu: !!fromContextMenu })) || {};
  } catch { return {}; } // 页面脚本还没加载（比如扩展刚装好、页面没刷新）
}

// 下载某个标签页里的视频。page：页面脚本已经找到的视频地址。
async function downloadTabVideo(tab, srcHint, page, fromContextMenu, direct) {
  if (blocked(tab?.url) || blocked(page) || blocked(srcHint)) return { ok: false, error: STORE_BLOCK_MSG };
  if (!page && !direct) ({ page, direct } = await askPage(tab.id, fromContextMenu));
  // 抖音：直接下载播放器正在放的 mp4（带上来源页和 Cookie）
  if (direct?.url) {
    const name = direct.title.replace(/[\\/:*?"<>|]/g, "_") + ".mp4";
    // HLS 流（.m3u8）不是单个文件，交给解析器下载并合并
    if (/\.m3u8(\?|$)/i.test(direct.url)) return sendToApp([{ url: direct.url, video: true }], tab.url);
    return sendToApp([{ url: direct.url, filename: name }], tab.url);
  }
  if (page) return sendToApp([{ url: page, video: true }], tab.url);
  if (isVideoSite(tab.url)) {
    return { ok: false, error: "没找到是哪个视频。请打开视频自己的页面，或者把鼠标移到视频上点「熊猫下载」" };
  }
  const list = mediaList(tab.id);
  const pick = list[0] || (srcHint && /^https?:/.test(srcHint) ? { url: srcHint, kind: "视频" } : null);
  if (!pick) return { ok: false, error: "这个页面还没有发现视频，先播放一下再试" };
  return sendToApp([{ url: pick.url, video: pick.kind === "HLS" || pick.kind === "DASH" }], tab.url);
}

// ───────── 3. 右键菜单 ─────────
chrome.runtime.onInstalled.addListener(() => {
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({ id: "link", title: "用熊猫下载器下载", contexts: ["link"] });
    chrome.contextMenus.create({ id: "media", title: "用熊猫下载器下载这个视频", contexts: ["video", "audio"] });
    chrome.contextMenus.create({ id: "pageVideo", title: "用熊猫下载器下载本页视频", contexts: ["page"] });
    chrome.contextMenus.create({ id: "allLinks", title: "用熊猫下载器下载本页所有文件", contexts: ["page"] });
  });
});

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  let r;
  if (info.menuItemId === "link") {
    const video = isVideoSite(info.linkUrl);
    r = blocked(info.linkUrl) ? { ok: false, error: STORE_BLOCK_MSG } : await sendToApp([{ url: info.linkUrl, video }], tab?.url);
  } else if (info.menuItemId === "media" || info.menuItemId === "pageVideo") {
    r = await downloadTabVideo(tab, info.srcUrl, null, info.menuItemId === "media");
  } else if (info.menuItemId === "allLinks") {
    const [res] = await chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: (pattern) => [...new Set([...document.querySelectorAll("a[href]")].map((a) => a.href))].filter((h) => new RegExp(pattern, "i").test(h)),
      args: [FILE_EXT.source],
    });
    const links = (res?.result || []).slice(0, 500);
    r = links.length ? await sendToApp(links.map((url) => ({ url })), tab.url) : { ok: false, error: "本页没有找到可下载的文件链接" };
  }
  if (r && !r.ok) notify(r.error);
});

function notify(msg) {
  chrome.action.setBadgeText({ text: "!" });
  chrome.action.setBadgeBackgroundColor({ color: "#e03e3e" });
  chrome.action.setTitle({ title: `熊猫下载器：${msg}` });
  setTimeout(() => { chrome.action.setBadgeText({ text: "" }); chrome.action.setTitle({ title: "熊猫下载器" }); }, 6000);
}

// ───────── 和弹出面板、页面脚本通信 ─────────
chrome.runtime.onMessage.addListener((msg, sender, reply) => {
  (async () => {
    if (msg.type === "alt") { altUntil = Date.now() + 2500; reply({ ok: true }); return; }
    if (msg.type === "state") {
      const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
      const ping = await native({ cmd: "ping" });
      reply({ connected: !!ping.ok, error: ping.error, version: ping.version, takeover: settings.takeover,
        tab: tab ? { id: tab.id, url: tab.url, title: tab.title, videoSite: isVideoSite(tab.url) } : null,
        media: tab ? mediaList(tab.id) : [] });
      return;
    }
    if (msg.type === "download") { reply(await sendToApp(msg.items, msg.referer)); return; }
    if (msg.type === "tabVideo") {
      const tab = sender.tab || (await chrome.tabs.query({ active: true, currentWindow: true }))[0];
      reply(await downloadTabVideo(tab, msg.src, msg.page, false, msg.direct));
      return;
    }
    if (msg.type === "show") { reply(await native({ cmd: "show" })); return; }
    reply({ ok: false });
  })();
  return true; // 异步回复
});
