// 熊猫下载器 · 页面脚本
// - 按住 Alt 点击链接：这一次交还浏览器下载
// - 鼠标移到网页视频上：右上角出现"熊猫下载"按钮
// - 帮后台找出"这个视频"自己的页面地址：首页、推荐流、搜索页里的预览播放，地址栏不是视频地址
(() => {
  if (window.top !== window || window.__pandaDl) return;
  window.__pandaDl = true;

  document.addEventListener("click", (e) => {
    if (e.altKey) chrome.runtime.sendMessage({ type: "alt" }).catch(() => {});
  }, true);

  // ───────── 找视频自己的页面地址 ─────────
  // 单个视频页面的地址规律
  const VIDEO_PAGE = [
    /youtube\.com\/watch\?(?:.*&)?v=[\w-]{11}/, /youtube\.com\/shorts\/[\w-]{11}/, /youtu\.be\/[\w-]{11}/,
    /bilibili\.com\/video\/(?:BV\w{10}|av\d+)/i, /bilibili\.com\/bangumi\/play\//,
    /douyin\.com\/video\/\d{15,}/, /[?&]modal_id=\d{15,}/,
    /ixigua\.com\/\d{15,}/, /kuaishou\.com\/short-video\//, /xiaohongshu\.com\/(?:explore|discovery\/item)\/\w+/,
    /(?:twitter|x)\.com\/[^/]+\/status\/\d+/, /vimeo\.com\/\d+/, /tiktok\.com\/@[^/]+\/video\/\d+/,
  ];
  const isVideoPage = (u) => VIDEO_PAGE.some((r) => r.test(u));
  const LINK_SEL = 'a[href*="/watch?v="], a[href*="/shorts/"], a[href*="/video/"], a[href*="modal_id="], a[href*="/status/"], a[href*="/explore/"]';

  // 最近一次鼠标经过的视频链接（YouTube 首页的预览播放器不在卡片里面，要靠它）
  let lastLink = null, lastLinkAt = 0;
  document.addEventListener("mouseover", (e) => {
    const a = e.target.closest?.("a[href]");
    if (a && isVideoPage(a.href)) { lastLink = a.href; lastLinkAt = Date.now(); }
  }, true);

  // 主域名相同（www.youtube.com 和 m.youtube.com 算同一个网站）
  function sameSite(a, b) {
    const root = (u) => { try { return new URL(u).hostname.split(".").slice(-2).join("."); } catch { return ""; } };
    return root(a) !== "" && root(a) === root(b);
  }

  function douyinId(v) {
    for (let el = v; el && el !== document.body; el = el.parentElement) {
      for (const a of ["data-e2e-vid", "data-vid", "data-aweme-id"]) {
        const x = el.getAttribute && el.getAttribute(a);
        if (x && /^\d{15,}$/.test(x)) return x;
      }
    }
    return null;
  }

  // 抖音、快手、西瓜：视频信息接口要签名，后台解析不了。网页自己拿到的数据由 site_hook.js 记下，这里按视频编号去取。
  const HOOK_SITE = /(^|\.)douyin\.com$/.test(location.hostname) ? "douyin"
    : /(^|\.)(kuaishou\.com|gifshow\.com)$/.test(location.hostname) ? "kuaishou"
    : /(^|\.)ixigua\.com$/.test(location.hostname) ? "xigua" : "";
  const SITE_NAME = { douyin: "抖音", kuaishou: "快手", xigua: "西瓜视频" };

  function askHook(id) {
    return new Promise((resolve) => {
      if (!id) return resolve(null);
      const nonce = Math.random().toString(36).slice(2);
      const done = (info) => { window.removeEventListener("message", on); clearTimeout(t); resolve(info); };
      const on = (e) => { if (e.source === window && e.data?.type === "panda-hook-info" && e.data.nonce === nonce) done(e.data.info); };
      const t = setTimeout(() => done(null), 800);
      window.addEventListener("message", on);
      window.postMessage({ type: "panda-hook-get", id, nonce }, "*");
    });
  }

  function cleanTitle(s) {
    return String(s || "").replace(/#\S+/g, "").replace(/@\S+/g, "").replace(/\s+/g, " ").trim().slice(0, 60);
  }

  // 这个视频在本站的编号
  function videoId(v) {
    if (HOOK_SITE === "douyin") {
      return (v && douyinId(v)) || (location.href.match(/(?:modal_id=|\/video\/)(\d{15,})/) || [])[1] || null;
    }
    const page = pageFor(v) || location.href;
    if (HOOK_SITE === "kuaishou") return (page.match(/\/(?:short-video|photo|fw\/photo)\/([\w-]+)/) || [])[1] || null;
    if (HOOK_SITE === "xigua") return (page.match(/ixigua\.com\/(?:video\/)?(\d{15,})/) || [])[1] || null;
    return null;
  }

  // 返回 { url, title } 或 null
  async function directVideo(v) {
    if (!HOOK_SITE) return null;
    const id = videoId(v);
    const info = await askHook(id);
    const name = SITE_NAME[HOOK_SITE];
    if (info && info.urls && info.urls.length) {
      const title = cleanTitle(info.desc) || `${name}视频_${id}`;
      return { url: info.urls[0], title: info.author ? `${title}_${info.author}`.slice(0, 80) : title };
    }
    // 退路：播放器直接放 mp4（不是 blob）时用它的地址
    if (!v) return null;
    const srcs = [v.currentSrc, v.src, ...[...v.querySelectorAll("source")].map((s) => s.src)];
    const url = srcs.find((s) => s && /^https?:/.test(s));
    if (!url) return null;
    let card = v;
    for (let i = 0; card && card !== document.body && i < 12; i++, card = card.parentElement) {
      if (card.getAttribute?.("data-e2e-vid") || card.querySelector?.('[data-e2e="video-desc"]')) break;
    }
    const desc = card?.querySelector?.('[data-e2e="video-desc"]')?.innerText || document.title.replace(/\s*[-|_]\s*(抖音|快手|西瓜视频).*$/, "");
    return { url, title: cleanTitle(desc) || `${name}视频_${id || Date.now()}` };
  }
  const area = (v) => { const r = v.getBoundingClientRect(); return Math.max(0, Math.min(r.bottom, innerHeight) - Math.max(r.top, 0)) * Math.max(0, r.width); };

  // 这个视频元素是不是页面的主播放器（而不是侧栏、片尾推荐里的小视频）
  function isMainPlayer(v) {
    if (v.closest("#movie_player, #shorts-player, .bpx-player-container, #bilibili-player, .player-container, [data-e2e='feed-active-video']")) return true;
    const vs = [...document.querySelectorAll("video")];
    return vs.every((x) => x === v || area(x) <= area(v)) && v.getBoundingClientRect().width > innerWidth * 0.4;
  }

  function pageFor(v) {
    if (!v) return isVideoPage(location.href) ? location.href : null;
    // 1. 抖音：视频卡片上直接写着编号
    const dy = douyinId(v);
    if (dy) return `https://www.douyin.com/video/${dy}`;
    // 2. 当前就是视频页，而且是主播放器
    if (isVideoPage(location.href) && isMainPlayer(v)) return location.href;
    // 3. 视频周围的链接（往外找几层，范围太大就停，免得拿到别的视频）
    const vr = v.getBoundingClientRect();
    for (let el = v, i = 0; el && el !== document.body && i < 8; el = el.parentElement, i++) {
      const up = el.closest?.("a[href]");
      if (up && isVideoPage(up.href)) return up.href;
      const r = el.getBoundingClientRect();
      if (r.width * r.height > vr.width * vr.height * 4 + 1) break;
      const a = [...el.querySelectorAll(LINK_SEL)].find((x) => isVideoPage(x.href));
      if (a) return a.href;
    }
    // 4. YouTube 首页预览：刚才鼠标经过的那条视频（只认同一个网站的链接）
    if (lastLink && Date.now() - lastLinkAt < 8000 && sameSite(lastLink, location.href)) return lastLink;
    // 5. 地址栏
    return isVideoPage(location.href) ? location.href : null;
  }

  // 后台（右键菜单、弹出面板）来问"现在是哪个视频"
  let rightClicked = null;
  document.addEventListener("contextmenu", (e) => {
    rightClicked = e.target instanceof HTMLVideoElement ? e.target : e.target.closest?.("video") || null;
  }, true);
  chrome.runtime.onMessage.addListener((msg, _sender, reply) => {
    if (msg.type !== "findPage") return;
    let v = msg.fromContextMenu ? rightClicked : null;
    if (!v) {
      const vs = [...document.querySelectorAll("video")].filter((x) => area(x) > 0);
      v = vs.find((x) => !x.paused) || vs.sort((a, b) => area(b) - area(a))[0] || null;
    }
    directVideo(v).then((direct) => reply({ page: pageFor(v), direct }));
    return true; // 异步回复
  });

  // ───────── 视频上的"熊猫下载"按钮 ─────────
  // 按钮放在 Shadow DOM 里，不受网页样式影响，也不影响网页
  const host = document.createElement("div");
  host.style.cssText = "position:fixed;z-index:2147483647;top:0;left:0;width:0;height:0;";
  const root = host.attachShadow({ mode: "closed" });
  root.innerHTML = `<style>
    .b{position:fixed;display:none;align-items:center;gap:6px;height:30px;padding:0 12px 0 8px;border-radius:15px;border:0;
       background:rgba(24,26,31,.82);color:#fff;font:500 12px/1 "Segoe UI","Microsoft YaHei UI",sans-serif;cursor:pointer;
       backdrop-filter:blur(12px);-webkit-backdrop-filter:blur(12px);box-shadow:0 4px 14px rgba(0,0,0,.25);transition:background .15s,transform .1s}
    .b:hover{background:rgba(18,184,134,.95)} .b:active{transform:scale(.97)}
    .b img{width:16px;height:16px;border-radius:4px}
    .b.busy{pointer-events:none;opacity:.85}
  </style><button class="b" title="用熊猫下载器下载这个视频"><img alt=""><span>熊猫下载</span></button>`;
  const btn = root.querySelector(".b");
  root.querySelector("img").src = chrome.runtime.getURL("icons/32.png");
  const label = root.querySelector("span");
  (document.body || document.documentElement).appendChild(host);

  let current = null, hideTimer = 0;
  function place(v) {
    const r = v.getBoundingClientRect();
    if (r.width < 240 || r.height < 140) return hide();
    btn.style.display = "flex";
    btn.style.top = Math.max(8, r.top + 10) + "px";
    btn.style.left = Math.min(window.innerWidth - btn.offsetWidth - 8, r.right - btn.offsetWidth - 10) + "px";
  }
  function hide() { btn.style.display = "none"; current = null; }

  document.addEventListener("mouseover", (e) => {
    const v = e.target instanceof HTMLVideoElement ? e.target : e.target.closest?.("video");
    const overBtn = e.composedPath().includes(host);
    if (v) { clearTimeout(hideTimer); current = v; place(v); }
    else if (!overBtn && current) { clearTimeout(hideTimer); hideTimer = setTimeout(hide, 800); }
  }, true);
  window.addEventListener("scroll", () => current && place(current), { passive: true });
  btn.addEventListener("mouseenter", () => clearTimeout(hideTimer));

  btn.addEventListener("click", async (e) => {
    e.preventDefault();
    e.stopPropagation();
    btn.classList.add("busy");
    label.textContent = "正在发送…";
    let r;
    try { const v = current;
      r = await chrome.runtime.sendMessage({ type: "tabVideo", src: v?.currentSrc || v?.src || "", page: pageFor(v), direct: await directVideo(v) }); }
    catch { r = { ok: false, error: "扩展已更新，请刷新页面" }; }
    label.textContent = r && r.ok ? "已交给熊猫下载器" : (r?.error || "发送失败");
    setTimeout(() => { label.textContent = "熊猫下载"; btn.classList.remove("busy"); }, 2200);
  });
})();
