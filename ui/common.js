// 各窗口共用的小工具。
const TAURI = window.__TAURI__;
const invoke = (cmd, args) => TAURI.core.invoke(cmd, args);
const listen = (ev, fn) => TAURI.event.listen(ev, (e) => fn(e.payload));
const $ = (sel, root = document) => root.querySelector(sel);

function human(n) {
  if (n == null) return "";
  const u = ["B", "KB", "MB", "GB", "TB"];
  let v = n, i = 0;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return i === 0 ? `${n} B` : `${v.toFixed(v >= 100 ? 0 : 1)} ${u[i]}`;
}
const speed = (n) => (n > 0 ? `${human(n)}/s` : "");
function eta(t) {
  if (!t.total || !t.speed) return "";
  const s = Math.max(0, Math.round((t.total - t.downloaded) / t.speed));
  if (s >= 3600) return `${Math.floor(s / 3600)} 小时 ${Math.floor((s % 3600) / 60)} 分`;
  if (s >= 60) return `${Math.floor(s / 60)} 分 ${s % 60} 秒`;
  return `${s} 秒`;
}
const pct = (t) => (t.total ? Math.min(100, (t.downloaded * 100) / t.total) : 0);
function esc(s) {
  return String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

// 窗口高度跟着内容走。
let fitTimer = 0;
let lastFit = "";
function fit(width) {
  cancelAnimationFrame(fitTimer);
  fitTimer = requestAnimationFrame(() => {
    const h = Math.ceil(document.body.getBoundingClientRect().height);
    const key = `${width}x${h}`;
    if (h > 0 && key !== lastFit) { lastFit = key; invoke("fit_window", { width, height: h }); }
  });
}

function toast(msg, ms = 2600) {
  let el = $(".toast");
  if (!el) { el = document.createElement("div"); el.className = "toast"; document.body.appendChild(el); }
  el.textContent = msg;
  el.classList.add("show");
  clearTimeout(el._t);
  el._t = setTimeout(() => el.classList.remove("show"), ms);
}

// 线条图标（不依赖外部字体）
const ICON = {
  pause: '<svg viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="5" width="4" height="14" rx="1.2"/><rect x="14" y="5" width="4" height="14" rx="1.2"/></svg>',
  play: '<svg viewBox="0 0 24 24" fill="currentColor"><path d="M8 5.5v13a1 1 0 0 0 1.5.86l10.5-6.5a1 1 0 0 0 0-1.72L9.5 4.64A1 1 0 0 0 8 5.5z"/></svg>',
  x: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>',
  minus: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M6 12h12"/></svg>',
  expand: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 9l6 6 6-6"/></svg>',
  folder: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linejoin="round"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>',
  retry: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 11a8 8 0 1 0-2.3 5.7"/><path d="M20 5v6h-6"/></svg>',
  check: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>',
  down: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 4v12M6 11l6 6 6-6M5 20h14"/></svg>',
  history: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 12a9 9 0 1 0 3-6.7L3 8"/><path d="M3 3v5h5M12 7v5l3 2"/></svg>',
};

// 文件类型图标（完成窗、主窗口用）
const CAT = {
  archive: ["#f2a83b", "压缩包"], video: ["#7a5cff", "视频"], audio: ["#ef5d8a", "音乐"],
  program: ["#3b82f6", "程序"], document: ["#10b98a", "文档"], image: ["#f06a3b", "图片"], other: ["#8a919c", "文件"],
};
function fileBadge(cat, size = 36) {
  const [color] = CAT[cat] || CAT.other;
  return `<svg width="${size}" height="${size}" viewBox="0 0 36 36"><path d="M9 3h12l8 8v20a2 2 0 0 1-2 2H9a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z" fill="${color}" opacity=".16"/><path d="M9 3h12l8 8v20a2 2 0 0 1-2 2H9a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z" fill="none" stroke="${color}" stroke-width="1.6"/><path d="M21 3v6a2 2 0 0 0 2 2h6" fill="none" stroke="${color}" stroke-width="1.6"/></svg>`;
}

Object.assign(ICON, {
  link: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/></svg>',
  gear: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/></svg>',
  inbox: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v10M8 9l4 4 4-4"/><path d="M4 14v4a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-4"/></svg>',
  done: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M8 12.5l2.7 2.7L16 10"/></svg>',
  trash: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"/></svg>',
  window: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 9h18"/></svg>',
  plus: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>',
});

// 路径从中间省略：E:\下载\…\压缩包，保住盘符和最后一级文件夹。按实际像素宽度计算。
const measureCtx = document.createElement("canvas").getContext("2d");
function midEllipsis(text, maxPx, font = "13px " + getComputedStyle(document.documentElement).getPropertyValue("--font")) {
  measureCtx.font = font;
  const w = (s) => measureCtx.measureText(s).width;
  if (!text || w(text) <= maxPx) return text || "";
  const sep = text.includes("\\") ? "\\" : "/";
  const parts = text.split(sep);
  const first = parts[0], last = parts[parts.length - 1];
  const middle = parts.slice(1, -1);
  // 保留开头和结尾，中间的文件夹从前往后逐级换成 …
  for (let keep = middle.length - 1; keep >= 0; keep--) {
    const s = [first, "…", ...middle.slice(middle.length - keep), last].join(sep);
    if (w(s) <= maxPx) return s;
  }
  // 最后一级本身太长：字符级中间省略
  let a = Math.floor(text.length / 2), b = a;
  while (a > 1 && w(text.slice(0, a) + "…" + text.slice(b)) > maxPx) { a--; b++; }
  return text.slice(0, a) + "…" + text.slice(b);
}
function renderPaths(root = document) {
  root.querySelectorAll("[data-path-text]").forEach((el) => {
    const full = el.dataset.pathText;
    el.textContent = midEllipsis(full, el.clientWidth - 2, getComputedStyle(el).font);
    el.title = full;
  });
}
window.addEventListener("resize", () => renderPaths());

// 去掉浏览器自带的行为：右键菜单、刷新、打印、查找、缩放、拖动图片。
document.addEventListener("contextmenu", (e) => {
  if (!e.target.closest("input, textarea")) e.preventDefault();
});
document.addEventListener("keydown", (e) => {
  const k = e.key.toLowerCase();
  const ctrl = e.ctrlKey || e.metaKey;
  if (k === "f5" || k === "f3" || k === "f7" || k === "f12" ||
      (ctrl && ["r", "p", "f", "g", "u", "s", "j", "h", "=", "+", "-", "0"].includes(k)) ||
      (ctrl && e.shiftKey && ["i", "j", "c", "r"].includes(k)) || (e.altKey && (k === "arrowleft" || k === "arrowright"))) {
    e.preventDefault();
  }
}, true);
document.addEventListener("wheel", (e) => { if (e.ctrlKey) e.preventDefault(); }, { passive: false });
document.addEventListener("dragstart", (e) => { if (!e.target.closest?.("[draggable=true]")) e.preventDefault(); });

// 增量更新：只改变化的部分，按钮等元素保持不变（整块重绘会让点击偶尔失效、悬停闪烁）。
function morph(target, html) {
  const tpl = document.createElement("template");
  tpl.innerHTML = html;
  morphChildren(target, tpl.content);
}
function morphChildren(a, b) {
  const an = [...a.childNodes], bn = [...b.childNodes];
  for (let i = 0; i < bn.length; i++) {
    const x = an[i], y = bn[i];
    if (!x) { a.appendChild(y); continue; }
    if (x.nodeType !== y.nodeType || x.nodeName !== y.nodeName || (x.dataset && y.dataset && x.dataset.key !== y.dataset.key)) {
      a.replaceChild(y, x); continue;
    }
    if (x.nodeType === 3) { if (x.data !== y.data) x.data = y.data; continue; }
    if (x.nodeType !== 1) continue;
    for (const { name, value } of [...y.attributes]) if (x.getAttribute(name) !== value) x.setAttribute(name, value);
    for (const { name } of [...x.attributes]) if (!y.hasAttribute(name)) x.removeAttribute(name);
    if (x.tagName === "INPUT") { if (x.checked !== y.checked) x.checked = y.checked; continue; }
    morphChildren(x, y);
  }
  for (let i = an.length - 1; i >= bn.length; i--) an[i].remove();
}

// 错误信息：第一行是给人看的原因，"详情："一行是原始信息（悬停或确认窗里显示）
const errFirst = (e) => String(e || "下载失败").split("\n")[0];
const errHtml = (e) => String(e || "").split("\n").map((l, i) => i ? `<div style="margin-top:4px;opacity:.75;font-size:11px;word-break:break-all">${esc(l)}</div>` : esc(l)).join("");

// 按钮事件统一用 data-act 委托（CSP 不允许内联 onclick）。
function onAct(root, handlers) {
  root.addEventListener("click", (e) => {
    const el = e.target.closest("[data-act]");
    if (!el || !root.contains(el)) return;
    const fn = handlers[el.dataset.act];
    if (fn) { e.stopPropagation(); fn(el.dataset.id ? Number(el.dataset.id) : undefined, el, e); }
  });
}
