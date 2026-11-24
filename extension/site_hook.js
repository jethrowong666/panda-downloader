// 熊猫下载器 · 网页钩子（运行在页面自己的环境里，只在抖音、快手、西瓜视频上启用）
// 这几个网站的视频信息接口都要签名，只有网页自己能调。这里在网页收到接口数据时顺手记下
// 每个视频的播放地址、标题、作者，扩展点"熊猫下载"时直接用。
(() => {
  if (window.__pandaHook) return;
  window.__pandaHook = true;
  const store = new Map(); // 视频编号 -> { id, urls, desc, author }
  const MAX = 800;
  const host = location.hostname;
  const SITE = /douyin\.com$/.test(host) ? "douyin" : /kuaishou\.com$|gifshow\.com$|kwai/.test(host) ? "kuaishou" : /ixigua\.com$/.test(host) ? "xigua" : "";

  const https = (u) => (typeof u === "string" ? u.replace(/^http:\/\//, "https://").replace(/^\/\//, "https://") : "");
  const uniq = (list) => [...new Set(list.map(https).filter((u) => /^https:\/\//.test(u)))];
  function save(id, urls, desc, author) {
    id = String(id || "");
    urls = uniq(urls);
    if (!id || !urls.length) return;
    const old = store.get(id);
    store.set(id, { id, urls, desc: desc || old?.desc || "", author: author || old?.author || "" });
    if (store.size > MAX) store.delete(store.keys().next().value);
  }

  // ── 抖音：aweme 对象 ──
  function douyin(o) {
    const id = o.aweme_id || o.awemeId;
    if (!id || !o.video || typeof o.video !== "object") return;
    const v = o.video;
    const rates = (Array.isArray(v.bit_rate) ? v.bit_rate : [])
      .filter((b) => b?.play_addr?.url_list?.length)
      .sort((a, b) => (a.is_h265 || 0) - (b.is_h265 || 0) || (b.bit_rate || 0) - (a.bit_rate || 0));
    const urls = [];
    for (const b of rates) urls.push(...b.play_addr.url_list);
    if (v.play_addr?.url_list) urls.push(...v.play_addr.url_list);
    save(id, urls, o.desc || o.preview_title, o.author?.nickname || o.author?.unique_id);
  }

  // ── 快手：photo 对象（GraphQL 返回 { photo: {...}, author: {...} }）──
  function kuaishou(o, parentAuthor) {
    const p = o.photoUrl || o.manifest || o.mainMvUrls ? o : null;
    if (!p) return;
    const id = p.id || p.photoId;
    const urls = [];
    // manifest 里有多个清晰度，按高度从高到低，H.264 优先
    const reps = [];
    for (const set of p.manifest?.adaptationSet || []) for (const r of set.representation || []) reps.push(r);
    reps.sort((a, b) => (/hevc|h265/i.test(a.codec || "") ? 1 : 0) - (/hevc|h265/i.test(b.codec || "") ? 1 : 0) || (b.height || 0) - (a.height || 0) || (b.avgBitrate || 0) - (a.avgBitrate || 0));
    for (const r of reps) { if (r.url) urls.push(r.url); for (const u of r.backupUrl || []) urls.push(u); }
    if (p.photoUrl) urls.push(p.photoUrl);
    for (const m of p.mainMvUrls || []) if (m?.url) urls.push(m.url);
    save(id, urls, p.caption || p.originCaption, parentAuthor || p.userName || p.author?.name);
  }

  // ── 西瓜：videoResource / video_list，地址是 Base64 ──
  function b64(u) {
    if (typeof u !== "string" || !u) return "";
    if (/^(https?:)?\/\//.test(u)) return u;
    try { const s = atob(u); return /^(https?:)?\/\//.test(s) ? s : ""; } catch { return ""; }
  }
  // 西瓜视频页自带的数据里不一定写视频编号，这时用地址里的编号
  const pageGid = (location.pathname.match(/\/(?:video\/)?(\d{15,})/) || [])[1] || "";
  function xigua(o, fromGlobals) {
    const lists = [];
    const res = o.videoResource;
    if (res) for (const k of ["normal", "dash", "dash_120fps"]) if (res[k]?.video_list) lists.push(res[k].video_list);
    if (o.video_list) lists.push(o.video_list);
    if (!lists.length) return;
    const id = o.group_id || o.gid || o.groupId || o.item_id || o.itemId || (fromGlobals && pageGid) || o.vid || res?.vid;
    const items = [];
    for (const l of lists) for (const k in l) if (l[k] && typeof l[k] === "object") items.push(l[k]);
    items.sort((a, b) => (b.vheight || b.vwidth || 0) - (a.vheight || a.vwidth || 0));
    const urls = [];
    for (const it of items) for (const key of ["main_url", "backup_url_1", "mainUrl", "backupUrl1"]) { const u = b64(it[key]); if (u) urls.push(u); }
    save(id, urls, o.title || o.abstract, o.user_info?.name || o.author?.name || o.userInfo?.name);
  }

  function harvest(obj, depth = 0, author = "", fromGlobals = false) {
    if (!obj || typeof obj !== "object" || depth > 12) return;
    if (Array.isArray(obj)) { for (const x of obj) harvest(x, depth + 1, author, fromGlobals); return; }
    if (SITE === "douyin") douyin(obj);
    else if (SITE === "kuaishou") { if (obj.photo && obj.author?.name) author = obj.author.name; kuaishou(obj, author); }
    else if (SITE === "xigua") xigua(obj, fromGlobals);
    for (const k in obj) {
      const v = obj[k];
      if (v && typeof v === "object") harvest(v, depth + 1, author, fromGlobals);
    }
  }

  const looksJson = (ct) => /json/i.test(ct || "");

  // fetch
  const origFetch = window.fetch;
  if (origFetch) {
    window.fetch = function (...args) {
      const p = origFetch.apply(this, args);
      p.then((r) => { if (looksJson(r.headers.get("content-type"))) r.clone().json().then((j) => harvest(j)).catch(() => {}); }).catch(() => {});
      return p;
    };
  }

  // XMLHttpRequest
  const X = window.XMLHttpRequest;
  if (X) {
    const send = X.prototype.send;
    X.prototype.send = function (...args) {
      this.addEventListener("load", () => {
        try {
          if (this.responseType === "json") harvest(this.response);
          else if ((!this.responseType || this.responseType === "text") && looksJson(this.getResponseHeader("content-type"))) harvest(JSON.parse(this.responseText));
        } catch {}
      });
      return send.apply(this, args);
    };
  }

  // 页面一开始就带的数据（西瓜的 _SSR_HYDRATED_DATA、快手的 __APOLLO_STATE__ 等），被问到时再扫一遍
  function scanGlobals() {
    for (const k of ["_SSR_HYDRATED_DATA", "__APOLLO_STATE__", "__INITIAL_STATE__", "INIT_STATE", "_ROUTER_DATA", "RENDER_DATA"]) {
      try { if (window[k] && typeof window[k] === "object") harvest(window[k], 0, "", true); } catch {}
    }
  }

  // 隔离环境里的页面脚本来问
  window.addEventListener("message", (e) => {
    if (e.source !== window || !e.data || e.data.type !== "panda-hook-get") return;
    let info = e.data.id ? store.get(String(e.data.id)) : null;
    if (!info) { scanGlobals(); info = e.data.id ? store.get(String(e.data.id)) : null; }
    window.postMessage({ type: "panda-hook-info", nonce: e.data.nonce, info: info || null }, "*");
  });
})();
