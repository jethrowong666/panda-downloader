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
    /(?:twitter|x)\.com\/[^/]+\/status\/\d+/, /vimeo\.com\/\d+/, /tiktok\.com\/@[^/]*\/video\/\d+/,
    /xiaohongshu\.com\/(?:explore|discovery\/item|search_result)\/[0-9a-f]{24}/,
    /weibo\.(?:com|cn)\/(?:\d+\/[A-Za-z0-9]{8,10}(?:[?#]|$)|detail\/\w+|status\/\w+|tv\/show\/[\d:]+)/,
    /instagram\.com\/(?:[\w.]+\/)?(?:p|reel|reels|tv)\/[\w-]+/, /reddit\.com\/r\/\w+\/comments\/\w+/, /redd\.it\/\w+/,
    /facebook\.com\/(?:watch\/?\?(?:.*&)?v=\d+|[^/?#]+\/videos\/(?:[^/?#]+\/)?\d+|reel\/\d+|video\.php\?(?:.*&)?v=\d+|share\/[vr]\/\w+)/, /fb\.watch\/\w+/,
    /threads\.(?:net|com)\/@[\w.]+\/post\/[\w-]+/, /zhihu\.com\/(?:zvideo|video)\/\d+/, /haokan\.baidu\.com\/v\?(?:.*&)?vid=\d+/,
    /vimeo\.com\/\d+/, /twitch\.tv\/(?:videos\/\d+|[\w]+\/clip\/[\w-]+)/, /clips\.twitch\.tv\/[\w-]+/, /dailymotion\.com\/video\/\w+/, /dai\.ly\/\w+/,
    /acfun\.cn\/v\/ac\d+/, /bsky\.app\/profile\/[^/]+\/post\/\w+/, /linkedin\.com\/(?:posts|feed\/update)\//, /tumblr\.com\/[^/]+\/\d+/,
    /pinterest\.[a-z.]+\/pin\/\d+/, /pin\.it\/\w+/, /vk\.com\/(?:video|clip)-?\d+_\d+/, /rumble\.com\/v\w+/, /kick\.com\/[^/]+\/clips?\//,
    /streamable\.com\/\w+/, /imgur\.com\/(?:gallery\/|a\/)?\w+/, /9gag\.com\/gag\/\w+/, /ted\.com\/talks\/\w+/, /nicovideo\.jp\/watch\/\w+/,
    /huya\.com\/video\/play\/\d+/, /v\.douyu\.com\/show\/\w+/, /bilibili\.tv\/[^/]+\/video\/\d+/, /snapchat\.com\/spotlight\/\w+/,
  ];
  const isVideoPage = (u) => VIDEO_PAGE.some((r) => r.test(u));
  const LINK_SEL = 'a[href*="/watch?v="], a[href*="/shorts/"], a[href*="/video/"], a[href*="modal_id="], a[href*="/status/"], a[href*="/explore/"], ' +
    'a[href*="/discovery/item/"], a[href*="/p/"], a[href*="/reel/"], a[href*="/comments/"], a[href*="weibo.com/"], a[href*="/detail/"], a[href*="/short-video/"], ' +
    'a[href*="/videos/"], a[href*="/watch/"], a[href*="/post/"], a[href*="/zvideo/"], a[href*="vid="], a[href*="/pin/"]';

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

  // 这些网站的视频信息接口要签名或登录，后台解析不了。网页自己拿到的数据由 site_hook.js 记下，这里按视频编号去取。
  // （B 站、Reddit 的音视频是分开的，网页里没有完整 mp4，走「视频页 + 浏览器登录状态 → yt-dlp」）
  const H = location.hostname;
  const HOOK_SITE = /(^|\.)douyin\.com$/.test(H) ? "douyin"
    : /(^|\.)(kuaishou\.com|gifshow\.com)$/.test(H) ? "kuaishou"
    : /(^|\.)ixigua\.com$/.test(H) ? "xigua"
    : /(^|\.)xiaohongshu\.com$/.test(H) ? "xhs"
    : /(^|\.)weibo\.(com|cn)$/.test(H) ? "weibo"
    : /(^|\.)tiktok\.com$/.test(H) ? "tiktok"
    : /(^|\.)instagram\.com$/.test(H) ? "instagram"
    : /(^|\.)threads\.(net|com)$/.test(H) ? "threads"
    : /(^|\.)(facebook\.com|fb\.watch)$/.test(H) ? "facebook"
    : /(^|\.)zhihu\.com$/.test(H) ? "zhihu"
    : /(^|\.)haokan\.baidu\.com$/.test(H) ? "haokan"
    : /(^|\.)vimeo\.com$/.test(H) ? "vimeo"
    : /(^|\.)(x|twitter)\.com$/.test(H) ? "twitter" : "";
  const SITE_NAME = { douyin: "抖音", kuaishou: "快手", xigua: "西瓜视频", xhs: "小红书", weibo: "微博", tiktok: "TikTok", instagram: "Instagram", twitter: "X", threads: "Threads", facebook: "Facebook", zhihu: "知乎", haokan: "好看视频", vimeo: "Vimeo" };

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
    if (HOOK_SITE === "xhs") return (page.match(/\/(?:explore|discovery\/item|search_result)\/([0-9a-f]{24})/) || [])[1] || null;
    if (HOOK_SITE === "weibo") return (page.match(/weibo\.(?:com|cn)\/(?:\d+\/([A-Za-z0-9]{8,10})|detail\/(\w+)|status\/(\w+)|tv\/show\/([\d:]+))/) || []).slice(1).find(Boolean) || null;
    if (HOOK_SITE === "tiktok") return (page.match(/\/video\/(\d+)/) || [])[1] || null;
    if (HOOK_SITE === "instagram") return (page.match(/\/(?:p|reel|reels|tv)\/([\w-]+)/) || [])[1] || null;
    if (HOOK_SITE === "twitter") return (page.match(/\/status\/(\d+)/) || [])[1] || null;
    if (HOOK_SITE === "threads") return (page.match(/\/post\/([\w-]+)/) || [])[1] || null;
    if (HOOK_SITE === "facebook") return (page.match(/(?:[?&]v=|\/videos\/(?:[^/?#]+\/)?|\/reel\/)(\d{6,})/) || [])[1] || null;
    if (HOOK_SITE === "zhihu") return (page.match(/\/(?:zvideo|video)\/(\d+)/) || [])[1] || null;
    if (HOOK_SITE === "haokan") return (page.match(/[?&]vid=(\d+)/) || [])[1] || null;
    if (HOOK_SITE === "vimeo") return (page.match(/vimeo\.com\/(?:.*\/)?(\d{5,})/) || [])[1] || null;
    return null;
  }

  // Reddit：新版播放器元素上的 packaged-media-json 里有带声音的完整 mp4（解析器要登录才能用）
  function redditDirect(v) {
    for (let el = v, i = 0; el && el !== document.body && i < 8; el = el.parentElement, i++) {
      const raw = el.getAttribute?.("packaged-media-json");
      if (!raw) continue;
      try {
        const j = JSON.parse(raw);
        const perms = (j.playbackMp4s?.permutations || []).filter((p) => p?.source?.url);
        perms.sort((a, b) => (b.source.dimensions?.height || 0) - (a.source.dimensions?.height || 0));
        if (!perms.length) return null;
        const post = el.closest("shreddit-post");
        const title = cleanTitle(post?.getAttribute("post-title") || document.title.replace(/\s*:\s*r\/\w+.*$/, ""));
        const author = post?.getAttribute("author") || "";
        return { url: perms[0].source.url, title: author ? `${title || "Reddit 视频"}_${author}`.slice(0, 80) : title || "Reddit 视频" };
      } catch { return null; }
    }
    return null;
  }

  // 返回 { url, title } 或 null
  async function directVideo(v) {
    if (/(^|\.)reddit\.com$/.test(H)) return v ? redditDirect(v) : null;
    if (!HOOK_SITE) return null;
    if (HOOK_SITE === "twitter") {
      // 推文本身、引用的推文……逐个去问，第一个有视频的就是
      for (const id of tweetIds(v)) {
        const info = await askHook(id);
        if (info?.urls?.length) {
          const title = cleanTitle(info.desc) || `X 视频_${id}`;
          return { url: info.urls[0], title: info.author ? `${title}_${info.author}`.slice(0, 80) : title };
        }
      }
      return null;
    }
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

  // 明确是主播放器的容器（YouTube、B 站等），这种播放器里常有片尾推荐链接，不能按旁边的链接算
  const MAIN_PLAYER = "#movie_player, #shorts-player, .bpx-player-container, #bilibili-player, .player-container, [data-e2e='feed-active-video']";
  // 这个视频元素是不是页面的主播放器（而不是侧栏、片尾推荐里的小视频）
  function isMainPlayer(v) {
    if (v.closest(MAIN_PLAYER)) return true;
    const vs = [...document.querySelectorAll("video")];
    return vs.every((x) => x === v || area(x) <= area(v)) && v.getBoundingClientRect().width > innerWidth * 0.4;
  }

  // X：视频嵌得很深，按所在推文（article）找编号：带时间的那条链接是推文本身，其次是里面别的推文链接（引用的推文），最后是地址栏
  function tweetIds(v) {
    const ids = [];
    const add = (href) => { const m = String(href || "").match(/\/status\/(\d+)/); if (m && !ids.includes(m[1])) ids.push(m[1]); };
    const art = v && (v.closest('article') || v.closest('[data-testid="tweet"]'));
    if (art) {
      for (const a of art.querySelectorAll('a[href*="/status/"]')) if (a.querySelector('time')) add(a.href);
      for (const a of art.querySelectorAll('a[href*="/status/"]')) add(a.href);
    }
    // 视频不在任何推文里（比如单独的视频页）才用地址栏的编号
    if (!ids.length) add(location.href);
    return ids;
  }

  function pageFor(v) {
    if (!v) return isVideoPage(location.href) ? location.href : null;
    if (HOOK_SITE === "twitter") {
      const ids = tweetIds(v);
      return ids.length ? `https://x.com/i/status/${ids[0]}` : null;
    }
    // 1. 抖音：视频卡片上直接写着编号
    const dy = douyinId(v);
    if (dy) return `https://www.douyin.com/video/${dy}`;
    // 2. 当前就是视频页，而且明确是主播放器
    if (isVideoPage(location.href) && v.closest(MAIN_PLAYER)) return location.href;
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
    // 4. 当前就是视频页，这个视频看起来是主视频（X 详情页、小红书详情等，周围没有别的视频链接）
    if (isVideoPage(location.href) && isMainPlayer(v)) return location.href;
    // 5. YouTube 首页预览：刚才鼠标经过的那条视频（只认同一个网站的链接）
    if (lastLink && Date.now() - lastLinkAt < 8000 && sameSite(lastLink, location.href)) return lastLink;
    // 6. 地址栏
    return isVideoPage(location.href) ? location.href : null;
  }

  // 后台（右键菜单、弹出面板）来问"现在是哪个视频"
  let rightClicked = null;
  document.addEventListener("contextmenu", (e) => {
    rightClicked = videoAt(e.clientX, e.clientY, e.target);
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
    // 有的网站（单页应用）会整块替换页面内容，按钮被移走了就放回去
    if (!host.isConnected) (document.body || document.documentElement).appendChild(host);
    btn.style.display = "flex";
    btn.style.top = Math.max(8, r.top + 10) + "px";
    btn.style.left = Math.min(window.innerWidth - btn.offsetWidth - 8, r.right - btn.offsetWidth - 10) + "px";
  }
  function hide() { btn.style.display = "none"; current = null; }

  // 鼠标所在位置的视频。X、YouTube、Instagram 等网站的视频上面盖着一层透明的控制层，
  // 鼠标碰到的是控制层、不是视频本身，所以按位置判断：落在哪个视频的范围里（有多个取最小的）。
  function videoAt(x, y, target) {
    const t = target instanceof HTMLVideoElement ? target : target?.closest?.("video");
    if (t) return t;
    let best = null, bestArea = Infinity;
    for (const v of document.querySelectorAll("video")) {
      const r = v.getBoundingClientRect();
      if (r.width < 240 || r.height < 140) continue;
      if (x < r.left || x > r.right || y < r.top || y > r.bottom) continue;
      const a = r.width * r.height;
      if (a < bestArea) { best = v; bestArea = a; }
    }
    return best;
  }

  let lastMove = 0;
  document.addEventListener("mousemove", (e) => {
    const now = Date.now();
    if (now - lastMove < 80) return; // 节流，不影响页面性能
    lastMove = now;
    if (e.composedPath().includes(host)) { clearTimeout(hideTimer); return; }
    const v = videoAt(e.clientX, e.clientY, e.target);
    if (v) { clearTimeout(hideTimer); current = v; place(v); }
    else if (current) { clearTimeout(hideTimer); hideTimer = setTimeout(hide, 800); }
  }, { capture: true, passive: true });
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
