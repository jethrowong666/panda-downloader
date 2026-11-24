// 熊猫下载器 · 网页钩子（运行在页面自己的环境里）
// 启用网站：抖音、快手、西瓜视频、小红书、微博、知乎、好看视频、TikTok、Instagram、Threads、Facebook、X(Twitter)
// 这些网站的视频信息接口大多要签名或登录，只有网页自己能调。这里在网页收到接口数据时顺手记下
// 每个视频的播放地址、标题、作者，扩展点"熊猫下载"时直接用。
(() => {
  if (window.__pandaHook) return;
  window.__pandaHook = true;
  const store = new Map(); // 视频编号 -> { id, urls, desc, author }
  const MAX = 1000;
  const host = location.hostname;
  const SITE =
    /douyin\.com$/.test(host) ? "douyin"
    : /kuaishou\.com$|gifshow\.com$|kwai/.test(host) ? "kuaishou"
    : /ixigua\.com$/.test(host) ? "xigua"
    : /xiaohongshu\.com$/.test(host) ? "xhs"
    : /weibo\.(com|cn)$/.test(host) ? "weibo"
    : /tiktok\.com$/.test(host) ? "tiktok"
    : /instagram\.com$|threads\.(net|com)$/.test(host) ? "instagram"
    : /facebook\.com$|fb\.watch$/.test(host) ? "facebook"
    : /zhihu\.com$/.test(host) ? "zhihu"
    : /haokan\.baidu\.com$/.test(host) ? "haokan"
    : /vimeo\.com$/.test(host) ? "vimeo"
    : /(^|\.)(x|twitter)\.com$/.test(host) ? "twitter"
    : "";

  const https = (u) => (typeof u === "string" ? u.replace(/^http:\/\//, "https://").replace(/^\/\//, "https://") : "");
  const uniq = (list) => [...new Set(list.map(https).filter((u) => /^https:\/\//.test(u)))];
  function save(ids, urls, desc, author) {
    urls = uniq(urls);
    if (!urls.length) return;
    for (let id of [].concat(ids)) {
      id = id == null ? "" : String(id);
      if (!id) continue;
      const old = store.get(id);
      store.set(id, { id, urls, desc: desc || old?.desc || "", author: author || old?.author || "" });
    }
    while (store.size > MAX) store.delete(store.keys().next().value);
  }
  const isH265 = (s) => /hevc|h265|hvc1|hev1|bytevc1/i.test(s || "");

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
    const reps = [];
    for (const set of p.manifest?.adaptationSet || []) for (const r of set.representation || []) reps.push(r);
    reps.sort((a, b) => (isH265(a.codec) ? 1 : 0) - (isH265(b.codec) ? 1 : 0) || (b.height || 0) - (a.height || 0) || (b.avgBitrate || 0) - (a.avgBitrate || 0));
    const urls = [];
    for (const r of reps) { if (r.url) urls.push(r.url); for (const u of r.backupUrl || []) urls.push(u); }
    if (p.photoUrl) urls.push(p.photoUrl);
    for (const m of p.mainMvUrls || []) if (m?.url) urls.push(m.url);
    save(p.id || p.photoId, urls, p.caption || p.originCaption, parentAuthor || p.userName || p.author?.name);
  }

  // ── 西瓜：videoResource / video_list，地址是 Base64 ──
  function b64(u) {
    if (typeof u !== "string" || !u) return "";
    if (/^(https?:)?\/\//.test(u)) return u;
    try { const s = atob(u); return /^(https?:)?\/\//.test(s) ? s : ""; } catch { return ""; }
  }
  // 视频页自带的数据里不一定写视频编号，这时用地址里的编号
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

  // ── 小红书：笔记 note / note_card，视频在 video.media.stream.{h264,h265,av1} ──
  const pageNoteId = (location.pathname.match(/\/(?:explore|discovery\/item|search_result)\/([0-9a-f]{24})/) || [])[1] || "";
  function xhsNote(n, id, fromGlobals) {
    const v = n?.video;
    if (!v || typeof v !== "object") return;
    const urls = [];
    // 原始无水印文件
    const key = v.consumer?.originVideoKey || v.consumer?.origin_video_key;
    if (key) urls.push(`https://sns-video-bd.xhscdn.com/${key}`);
    const s = v.media?.stream || {};
    const streams = [...(s.h264 || []), ...(s.h265 || []), ...(s.av1 || []), ...(s.h266 || [])]
      .filter((x) => x && (x.masterUrl || x.master_url))
      .sort((a, b) => (isH265(a.videoCodec || a.video_codec) ? 1 : 0) - (isH265(b.videoCodec || b.video_codec) ? 1 : 0) || (b.height || 0) - (a.height || 0));
    for (const x of streams) { urls.push(x.masterUrl || x.master_url); for (const u of x.backupUrls || x.backup_urls || []) urls.push(u); }
    const nid = id || n.noteId || n.note_id || (fromGlobals && pageNoteId);
    save(nid, urls, n.title || n.displayTitle || n.display_title || n.desc, n.user?.nickname || n.user?.nickName || n.user?.nick_name);
  }
  function xhs(o, fromGlobals) {
    if (o.note_card) xhsNote(o.note_card, o.id || o.note_id, fromGlobals);
    else if (o.noteCard) xhsNote(o.noteCard, o.id || o.noteId, fromGlobals);
    else if (o.video && (o.noteId || o.note_id || o.type === "video")) xhsNote(o, null, fromGlobals);
  }

  // ── 微博：微博对象 page_info.media_info ──
  function weibo(o) {
    const mi = o.page_info?.media_info || o.pageInfo?.mediaInfo;
    if (!mi) return;
    const urls = [];
    const pl = (mi.playback_list || []).filter((p) => p?.play_info?.url).sort((a, b) => (b.play_info.height || 0) - (a.play_info.height || 0));
    for (const p of pl) urls.push(p.play_info.url);
    for (const k of ["mp4_1080p_mp4", "mp4_720p_mp4", "mp4_hd_url", "hevc_mp4_hd", "mp4_sd_url", "stream_url_hd", "stream_url"]) if (mi[k]) urls.push(mi[k]);
    const text = (o.text_raw || o.text || "").replace(/<[^>]+>/g, "");
    save([o.mblogid, o.idstr, o.mid, o.page_info?.object_id], urls, text || mi.title || mi.name, o.user?.screen_name);
  }

  // ── TikTok：itemStruct / itemList 里的 item ──
  function tiktok(o) {
    const v = o.video;
    if (!o.id || !v || typeof v !== "object" || !(v.playAddr || v.bitrateInfo || v.PlayAddrStruct)) return;
    const urls = [];
    const br = (v.bitrateInfo || []).filter((b) => b?.PlayAddr?.UrlList?.length)
      .sort((a, b) => (isH265(a.CodecType) ? 1 : 0) - (isH265(b.CodecType) ? 1 : 0) || (b.Bitrate || 0) - (a.Bitrate || 0));
    for (const b of br) urls.push(...b.PlayAddr.UrlList);
    if (typeof v.playAddr === "string") urls.push(v.playAddr);
    if (typeof v.downloadAddr === "string") urls.push(v.downloadAddr); // 带水印，放最后
    save(o.id, urls, o.desc, typeof o.author === "string" ? o.author : o.author?.uniqueId || o.author?.nickname);
  }

  // ── Instagram：media 对象 video_versions（新接口）/ video_url（旧接口）──
  function instagram(o) {
    let vv = o.video_versions;
    if ((!vv || !vv.length) && Array.isArray(o.carousel_media)) vv = o.carousel_media.find((c) => c?.video_versions?.length)?.video_versions;
    const urls = [];
    if (Array.isArray(vv) && vv.length) {
      for (const x of vv.slice().sort((a, b) => (b.width || 0) * (b.height || 0) - (a.width || 0) * (a.height || 0))) if (x?.url) urls.push(x.url);
    } else if (o.is_video && o.video_url) {
      urls.push(o.video_url);
    } else return;
    const caption = o.caption?.text || o.edge_media_to_caption?.edges?.[0]?.node?.text || "";
    save([o.code, o.shortcode, o.pk, o.id], urls, caption, o.user?.username || o.owner?.username);
  }

  // ── X / Twitter：推文 legacy.extended_entities.media[].video_info.variants ──
  function twitterTweet(id, legacy, author) {
    const media = legacy?.extended_entities?.media || [];
    const m = media.find((x) => x?.video_info?.variants?.length);
    if (!m) return;
    const urls = m.video_info.variants.filter((x) => x.content_type === "video/mp4" && x.url)
      .sort((a, b) => (b.bitrate || 0) - (a.bitrate || 0)).map((x) => x.url);
    const text = (legacy.full_text || legacy.text || "").replace(/https?:\/\/t\.co\/\w+/g, "").trim();
    save(id, urls, text, author);
  }
  function twitter(o) {
    if (o.rest_id && o.legacy?.extended_entities) {
      const u = o.core?.user_results?.result;
      twitterTweet(o.rest_id, o.legacy, u?.legacy?.screen_name || u?.core?.screen_name);
    } else if (o.id_str && o.extended_entities && !o.legacy) {
      twitterTweet(o.id_str, o, o.user?.screen_name);
    }
  }

  // ── Facebook：Video 对象（browser_native_* 是带声音的完整 mp4）──
  function facebook(o) {
    const f = o.videoDeliveryLegacyFields && typeof o.videoDeliveryLegacyFields === "object" ? { ...o.videoDeliveryLegacyFields, ...o } : o;
    const urls = [f.browser_native_hd_url, f.playable_url_quality_hd, f.hd_src, f.browser_native_sd_url, f.playable_url, f.sd_src].filter((u) => typeof u === "string");
    if (!urls.length) return;
    const id = o.videoId || o.video_id || o.id;
    const title = o.title?.text || o.savable_title?.text || o.name || o.message?.text || "";
    save([id, o.post_id], urls, title, o.owner?.name || o.video_owner?.name);
  }

  // ── 知乎：playlist.{FHD,HD,SD,LD}.play_url，zvideo 外面还包一层 ──
  function zhihu(o) {
    const pl = o.playlist || o.video?.playlist || o.playlist_v2 || o.video?.playlist_v2;
    if (!pl || typeof pl !== "object") return;
    const order = ["FHD", "fhd", "HD", "hd", "SD", "sd", "LD", "ld"];
    const urls = order.map((k) => pl[k]?.play_url || pl[k]?.url).filter(Boolean);
    if (!urls.length) for (const k in pl) if (pl[k]?.play_url) urls.push(pl[k].play_url);
    save([o.id, o.zvideo_id, o.video_id, o.video?.video_id], urls, o.title || o.video?.title, o.author?.name);
  }

  // ── 好看视频：clarityUrl 多清晰度 ──
  function haokan(o) {
    if (!Array.isArray(o.clarityUrl) || !o.clarityUrl.length) return;
    const rank = { "1080p": 5, sc: 4, "720p": 3, hd: 2, sd: 1 };
    const list = o.clarityUrl.filter((x) => x?.url).sort((a, b) => (rank[b.key] || 0) - (rank[a.key] || 0));
    save([o.vid, o.id], list.map((x) => x.url).concat(o.playurl ? [o.playurl] : []), o.title, o.author || o.author_name || o.mth?.author_name);
  }

  // ── Vimeo：播放器配置 request.files，优先 progressive（带声音的 mp4），没有就用 HLS ──
  function vimeo(o) {
    const files = o.request?.files;
    const vid = o.video?.id;
    if (!files || !vid) return;
    const urls = (files.progressive || []).filter((p) => p?.url).sort((a, b) => (b.height || 0) - (a.height || 0)).map((p) => p.url);
    if (!urls.length && files.hls?.cdns) {
      const cdns = files.hls.cdns;
      const def = cdns[files.hls.default_cdn] || Object.values(cdns)[0];
      if (def?.url) urls.push(def.url);
    }
    save(vid, urls, o.video.title, o.video.owner?.name);
  }

  function harvest(obj, depth = 0, author = "", fromGlobals = false) {
    if (!obj || typeof obj !== "object" || depth > 14) return;
    if (Array.isArray(obj)) { for (const x of obj) harvest(x, depth + 1, author, fromGlobals); return; }
    try {
      if (SITE === "douyin") douyin(obj);
      else if (SITE === "kuaishou") { if (obj.photo && obj.author?.name) author = obj.author.name; kuaishou(obj, author); }
      else if (SITE === "xigua") xigua(obj, fromGlobals);
      else if (SITE === "xhs") xhs(obj, fromGlobals);
      else if (SITE === "weibo") weibo(obj);
      else if (SITE === "tiktok") tiktok(obj);
      else if (SITE === "instagram") instagram(obj);
      else if (SITE === "twitter") twitter(obj);
      else if (SITE === "facebook") facebook(obj);
      else if (SITE === "zhihu") zhihu(obj);
      else if (SITE === "haokan") haokan(obj);
      else if (SITE === "vimeo") vimeo(obj);
    } catch {}
    for (const k in obj) {
      let v;
      try { v = obj[k]; } catch { continue; }
      if (v && typeof v === "object") harvest(v, depth + 1, author, fromGlobals);
    }
  }

  // 接口返回：JSON，也有 Instagram 那样标成 text/javascript、Facebook 系前面加 for (;;); 的
  const maybeJson = (ct) => /json|javascript|text\/plain/i.test(ct || "") || (SITE === "facebook" && /text\/html/i.test(ct || ""));
  function parseLoose(text) {
    if (!text || text.length > 20e6) return null;
    const t = text.replace(/^\s*for\s*\(;;\);/, "").trim();
    if (!/^[\[{]/.test(t)) return null;
    try { return JSON.parse(t); } catch {}
    // 一次返回多行 JSON（Facebook 等）
    const parts = [];
    for (const line of t.split(/\n(?=[\[{])/)) { try { parts.push(JSON.parse(line)); } catch {} }
    return parts.length ? parts : null;
  }

  // fetch
  const origFetch = window.fetch;
  if (origFetch) {
    window.fetch = function (...args) {
      const p = origFetch.apply(this, args);
      p.then((r) => { if (maybeJson(r.headers.get("content-type"))) r.clone().text().then((t) => harvest(parseLoose(t))).catch(() => {}); }).catch(() => {});
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
          else if ((!this.responseType || this.responseType === "text") && maybeJson(this.getResponseHeader("content-type"))) harvest(parseLoose(this.responseText));
        } catch {}
      });
      return send.apply(this, args);
    };
  }

  // 页面一开始就带的数据，被问到时再扫一遍：全局变量，以及 TikTok 那样放在 <script> 里的 JSON
  function scanGlobals() {
    for (const k of ["_SSR_HYDRATED_DATA", "__APOLLO_STATE__", "__INITIAL_STATE__", "__INITIAL_DATA__", "__PRELOADED_STATE__", "playerConfig", "INIT_STATE", "_ROUTER_DATA", "RENDER_DATA", "SIGI_STATE", "$render_data"]) {
      try { if (window[k] && typeof window[k] === "object") harvest(window[k], 0, "", true); } catch {}
    }
    for (const id of ["__UNIVERSAL_DATA_FOR_REHYDRATION__", "SIGI_STATE", "__NEXT_DATA__", "RENDER_DATA", "js-initialData"]) {
      const el = document.getElementById(id);
      if (!el) continue;
      let t = el.textContent || "";
      if (id === "RENDER_DATA") try { t = decodeURIComponent(t); } catch {}
      harvest(parseLoose(t), 0, "", true);
    }
    // Instagram、X 等把首屏数据放在 type="application/json" 的 <script> 里
    for (const el of document.querySelectorAll('script[type="application/json"]')) {
      const t = el.textContent || "";
      if (t.length > 200 && /video_versions|video_info|play_addr|masterUrl|media_info|browser_native|playable_url|play_url|clarityUrl/.test(t)) harvest(parseLoose(t), 0, "", true);
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
