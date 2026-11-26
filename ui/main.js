// 主窗口：侧边栏（分类、实时速度）+ 正在下载 / 已完成 / 设置。
let tab = "active";
let tasks = [];
let history = [];
let settings = null;
let version = "";
let flt = "all";          // 正在下载的筛选：all | running | paused
let cat = "";             // 已完成的分类筛选（空 = 全部）
let query = "";           // 已完成的搜索词
const samples = [];       // 最近一分钟的总速度，画曲线用

// Win11 有系统毛玻璃时窗口是透明的
if (new URLSearchParams(location.search).get("mica")) document.documentElement.classList.add("mica");

$("#linkIcon").innerHTML = ICON.link;
$("#addBtn").innerHTML = `${ICON.plus}新建`;

const TITLES = { active: "正在下载", done: "已完成", settings: "设置" };
const CAT_ORDER = ["video", "audio", "archive", "program", "document", "image", "other"];

// 正在进行的任务（已取消、等着删除的不算，它们只出现在撤销条里）
const activeTasks = () => tasks.filter((t) => t.confirmed && t.status !== "done" && t.status !== "cancelled");
const cancelledTasks = () => tasks.filter((t) => t.confirmed && t.status === "cancelled");

function renderNav() {
  const active = activeTasks();
  $("#navActive").innerHTML = `${ICON.inbox}<span>正在下载</span><span class="n num">${active.length || ""}</span>`;
  $("#navDone").innerHTML = `${ICON.done}<span>已完成</span><span class="n num">${history.length || ""}</span>`;
  $("#navSet").innerHTML = `${ICON.gear}<span>设置</span>`;
  // 分类：已完成文件按类型分
  const counts = {};
  history.forEach((h) => { const c = catOf(nameOf(h)); counts[c] = (counts[c] || 0) + 1; });
  const cats = CAT_ORDER.filter((c) => counts[c]);
  morph($("#cats"), cats.length ? `<div class="sep">分类</div>` + cats.map((c) =>
    `<div class="nav sub ${tab === "done" && cat === c ? "on" : ""}" data-act="cat" data-cat="${c}" data-key="c${c}"><i class="cdot" style="background:${CAT[c][0]}"></i><span>${CAT[c][1]}</span><span class="n num">${counts[c]}</span></div>`).join("") : "");
  $("#navActive").classList.toggle("on", tab === "active");
  $("#navDone").classList.toggle("on", tab === "done" && !cat);
  $("#navSet").classList.toggle("on", tab === "settings");
  $("#pageTitle").textContent = tab === "done" && cat ? CAT[cat][1] : TITLES[tab];
  $("#addBox").hidden = tab === "settings";
}

// 侧栏的速度数字和曲线
function renderGauge() {
  const sp = activeTasks().reduce((a, t) => a + (t.status === "downloading" ? t.speed : 0), 0);
  const [v, u] = (speed(sp) || "0 B/s").split(" ");
  $("#sideSpeed").textContent = v;
  $("#sideUnit").textContent = u;
  samples.push(sp);
  if (samples.length > 60) samples.shift();
  const max = Math.max(...samples, 1);
  const n = samples.length;
  if (n < 2 || max <= 1) { $("#sparkLine").setAttribute("d", "M0 32H200"); $("#sparkFill").setAttribute("d", ""); return; }
  const pts = samples.map((s, i) => [(i / 59) * 200 + (200 - ((n - 1) / 59) * 200), 31 - (s / max) * 27]);
  // 平滑曲线：相邻两点的中点做控制点
  let d = `M${pts[0][0].toFixed(1)} ${pts[0][1].toFixed(1)}`;
  for (let i = 1; i < n; i++) {
    const [x0, y0] = pts[i - 1], [x1, y1] = pts[i], mx = (x0 + x1) / 2;
    d += ` C${mx.toFixed(1)} ${y0.toFixed(1)} ${mx.toFixed(1)} ${y1.toFixed(1)} ${x1.toFixed(1)} ${y1.toFixed(1)}`;
  }
  $("#sparkLine").setAttribute("d", d);
  $("#sparkFill").setAttribute("d", `${d} V34 H${pts[0][0].toFixed(1)}z`);
}

function activeRow(t) {
  const p = pct(t);
  const size = `<span class="num">${human(t.downloaded)}${t.total ? " / " + human(t.total) : ""}</span>`;
  const btTag = t.bt ? (t.bt.web ? '<span class="tag" title="正在用种子附带的 HTTP 地址加速">HTTP 加速</span>'
    : `<span class="tag o num" title="已连上的 BT 用户">BT · ${t.bt.peers} 人</span>`) : "";
  let sub, barCls = "", left = "", right = "";
  switch (t.status) {
    case "failed":
      sub = `<span class="err" title="${esc(t.error || "")}">${esc(errFirst(t.error))}</span>`; barCls = "failed"; left = "<b>失败</b>"; right = "点重试再试一次"; break;
    case "paused":
      sub = `${size}<span class="tag g">已暂停</span>`; barCls = "paused";
      left = t.total ? `<b>${p.toFixed(1)}%</b>` : ""; right = t.resumable ? "可续传" : ""; break;
    default:
      if (t.video?.merging) sub = "<span>正在合并音视频…</span>";
      else if (t.video?.resolving) sub = "<span>正在解析视频…</span>";
      else if (t.bt?.resolving) sub = "<span>正在获取种子信息…</span>";
      else if (t.probing) sub = "<span>正在连接…</span>";
      else sub = size + btTag;
      left = `<b>${speed(t.speed) || (t.total ? p.toFixed(1) + "%" : "等待数据…")}</b>`;
      right = t.total && t.speed ? `剩余 ${eta(t)}` : t.total ? `${p.toFixed(1)}%` : "";
      if (!t.total) barCls = "indeterminate";
  }
  const ops = [];
  if (t.status === "downloading") ops.push(`<button class="icon" data-act="pause" data-id="${t.id}" title="暂停">${ICON.pause}</button>`);
  if (t.status === "paused") ops.push(`<button class="icon" data-act="resume" data-id="${t.id}" title="继续">${ICON.play}</button>`);
  if (t.status === "failed") ops.push(`<button class="icon" data-act="resume" data-id="${t.id}" title="重试">${ICON.retry}</button>`);
  ops.push(`<button class="icon" data-act="folder" data-id="${t.id}" title="打开所在文件夹">${ICON.folder}</button>`);
  ops.push(`<button class="icon danger" data-act="cancel" data-id="${t.id}" title="删除任务和已下载的数据（可撤销）">${ICON.trash}</button>`);
  return `<div class="t" data-key="a${t.id}">${badgeOf(t, 44)}
    <div class="info"><div class="name ellipsis" title="${esc(t.name)}">${esc(t.name)}</div><div class="sub">${sub}</div></div>
    <div class="prog"><div class="bar ${barCls}"><i style="width:${t.total ? p : 0}%"></i></div><div class="l num"><span>${left}</span><span>${right}</span></div></div>
    <div class="ops">${ops.join("")}</div></div>`;
}

const nameOf = (h) => h.path.split(/[\\/]/).pop();
function catOf(name) {
  const e = (name.split(".").pop() || "").toLowerCase();
  const m = { archive: "zip rar 7z tar gz xz bz2 iso", video: "mp4 mkv avi mov wmv flv webm ts m4v", audio: "mp3 flac wav aac m4a ogg opus", program: "exe msi apk dmg msix", document: "pdf doc docx xls xlsx ppt pptx txt epub md", image: "jpg jpeg png gif webp bmp svg heic" };
  return Object.keys(m).find((k) => m[k].split(" ").includes(e)) || "other";
}

function when(sec) {
  const d = new Date(sec * 1000), now = new Date();
  const hm = d.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
  if (d.toDateString() === now.toDateString()) return `今天 ${hm}`;
  const y = new Date(now); y.setDate(now.getDate() - 1);
  if (d.toDateString() === y.toDateString()) return `昨天 ${hm}`;
  return `${d.getMonth() + 1}月${d.getDate()}日 ${hm}`;
}

function doneRow(h) {
  const name = nameOf(h);
  const folder = h.path.slice(0, h.path.length - name.length - 1);
  const i = history.indexOf(h);
  return `<div class="t h" data-key="${esc(h.path)}">${fileBadge(catOf(name), 44)}
    <div class="info"><div class="name ellipsis" title="${esc(name)}">${esc(name)}</div><div class="sub num"><span>${human(h.size)}</span><span class="dot">${when(h.finished_at)}</span></div></div>
    <div class="where" data-path-text="${esc(folder)}"></div>
    <div class="ops"><button class="link" data-act="openPath" data-i="${i}">打开</button>
      <button class="icon" data-act="revealPath" data-i="${i}" title="打开所在文件夹">${ICON.folder}</button>
      <button class="icon danger" data-act="forget" data-i="${i}" title="从列表中移除（不删除文件）">${ICON.trash}</button></div></div>`;
}

function empty(title, lines) {
  return `<div class="empty"><img src="logo.png" alt=""><b>${title}</b>${lines.map((l) => `<p>${l}</p>`).join("")}</div>`;
}

function render() {
  renderNav();
  renderUndo();
  const main = $("#main"), tools = $("#tools");
  if (tab === "active") {
    const all = activeTasks();
    const running = all.filter((t) => t.status === "downloading");
    const paused = all.filter((t) => t.status === "paused" || t.status === "failed");
    if (flt === "running" && !running.length) flt = "all";
    if (flt === "paused" && !paused.length) flt = "all";
    const list = flt === "running" ? running : flt === "paused" ? paused : all;
    const chip = (k, label, n) => `<span class="f ${flt === k ? "on" : ""} num" data-act="flt" data-flt="${k}" data-key="f${k}">${label} ${n}</span>`;
    // 全部任务大约还要多久
    const sized = running.filter((t) => t.total && t.speed);
    const left = sized.length ? Math.max(...sized.map((t) => (t.total - t.downloaded) / t.speed)) : 0;
    const leftText = left >= 3600 ? `剩余约 ${Math.ceil(left / 3600)} 小时` : left >= 60 ? `剩余约 ${Math.ceil(left / 60)} 分钟` : left > 0 ? "马上完成" : "";
    morph(tools, all.length
      ? `${chip("all", "全部", all.length)}${running.length ? chip("running", "下载中", running.length) : ""}${paused.length ? chip("paused", "已暂停", paused.length) : ""}
         <span class="sum num">${leftText}</span>
         ${paused.length ? `<button data-act="resumeAll">${ICON.play}全部开始</button>` : ""}${running.length ? `<button data-act="pauseAll">${ICON.pause}全部暂停</button>` : ""}`
      : "");
    morph(main, list.length ? `<div class="list">${list.map(activeRow).join("")}</div>`
      : empty("没有正在下载的任务", ["复制一个下载链接或磁力链接，确认窗口会自动弹出", "也可以按 <kbd>Ctrl</kbd> <kbd>Alt</kbd> <kbd>D</kbd> 下载剪贴板里的链接"]));
  } else if (tab === "done") {
    const q = query.trim().toLowerCase();
    const list = history.filter((h) => (!cat || catOf(nameOf(h)) === cat) && (!q || nameOf(h).toLowerCase().includes(q)));
    if (!$("#find")) {
      tools.innerHTML = `<span class="sum num" id="doneSum" style="margin-left:0"></span><div class="find">${ICON.search}<input type="text" id="find" placeholder="搜索文件名" spellcheck="false"></div>`;
      $("#find").value = query;
      $("#find").addEventListener("input", (e) => { query = e.target.value; render(); });
    }
    $("#doneSum").textContent = history.length ? (list.length === history.length ? `共 ${history.length} 个文件` : `找到 ${list.length} 个，共 ${history.length} 个`) : "";
    $(".find").hidden = !history.length;
    main.innerHTML = list.length ? `<div class="list">${list.map(doneRow).join("")}</div>`
      : history.length ? empty("没有找到符合的文件", ["换个关键词，或者点左边的「已完成」看全部"])
      : empty("还没有下载完成的文件", ["下载完成的文件会出现在这里"]);
    requestAnimationFrame(() => renderPaths(main));
  } else {
    tools.innerHTML = "";
    renderSettings(main);
  }
}

// 删除后的撤销条：任务立刻从列表消失，这里说明删了什么，倒计时结束前可以撤销
function renderUndo() {
  const gone = cancelledTasks();
  const el = $("#undo");
  el.classList.toggle("show", gone.length > 0);
  if (!gone.length) return;
  const bytes = gone.reduce((a, t) => a + t.downloaded, 0);
  const what = gone.length === 1 ? `已删除「${esc(gone[0].name)}」` : `已删除 ${gone.length} 个任务`;
  $("#undoMsg").innerHTML = `${what}<small>${bytes > 0 ? `已下载的 ${human(bytes)} 会一并清除` : "还没下载到数据"}</small>`;
  const ms = Math.max(...gone.map((t) => t.undo_ms || 0));
  const total = Math.max(...gone.map((t) => t.undo_total_ms || 5000));
  $("#undoTm").style.width = `${Math.min(100, (ms / total) * 100)}%`;
}

function renderSettings(main) {
  if (!settings) return;
  const s = settings;
  const sw = (k) => `<div class="switch ${s[k] ? "on" : ""}" data-act="toggle" data-key="${k}" role="switch" aria-checked="${!!s[k]}"></div>`;
  const row = (title, desc, ctl) => `<div class="s"><div class="txt"><b>${title}</b>${desc ? `<small>${desc}</small>` : ""}</div><div class="ctl">${ctl}</div></div>`;
  main.innerHTML = `
    <div class="group"><h3>下载</h3><div class="box">
      ${row("下载位置", "视频、压缩包、程序等会自动放进对应的子文件夹", `<div class="path" data-act="pickBase">${ICON.folder}<span class="p" data-path-text="${esc(s.base_dir)}"></span><span class="change">更改</span></div>`)}
      ${row("每个任务的连接数", "连接越多通常越快，个别服务器会限制连接数", `<input type="number" id="conns" min="1" max="32" value="${s.connections}" style="width:88px">`)}
      ${row("全局限速", "所有任务加起来的最高速度，0 表示不限速；改完马上生效", `<input type="number" id="limit" min="0" step="0.5" value="${+(s.speed_limit / 1048576).toFixed(1)}" style="width:88px"><span class="unit">MB/s</span>`)}
    </div></div>
    <div class="group"><h3>外观</h3><div class="box">
      ${row("主题", "深色模式下所有窗口一起变暗", `<div class="segc">${[["system", "跟随系统"], ["light", "浅色"], ["dark", "深色"]].map(([k, l]) => `<span class="${(s.theme || "system") === k ? "on" : ""}" data-act="theme" data-key="${k}">${l}</span>`).join("")}</div>`)}
    </div></div>
    <div class="group"><h3>使用习惯</h3><div class="box">
      ${row("下载前确认", "关闭后复制链接就直接开始下载，不再弹出确认窗口", sw("ask_before_download"))}
      ${row("自动识别复制的链接", "复制下载链接、磁力链接时自动弹出；复制普通网页地址不会打扰你", sw("watch_clipboard"))}
      ${row("启动后继续未完成的任务", "电脑重启或软件退出后，下次打开自动从断点接着下载", sw("auto_resume"))}
    </div></div>
    <div class="group"><h3>快捷键</h3><div class="box">
      ${row("下载剪贴板里的链接", "任何时候都可以使用", "<span><kbd>Ctrl</kbd> <kbd>Alt</kbd> <kbd>D</kbd></span>")}
    </div></div>
    <div class="about">熊猫下载器 ${esc(version)} · 开源软件（GPL-3.0）</div>`;
  requestAnimationFrame(() => renderPaths(main));
  $("#conns").addEventListener("change", (e) => { const v = Math.max(1, Math.min(32, Math.round(+e.target.value) || 8)); e.target.value = v; save({ connections: v }); });
  $("#limit").addEventListener("change", (e) => { const v = Math.max(0, +e.target.value || 0); e.target.value = v; save({ speed_limit: Math.round(v * 1048576) }); });
}

async function save(patch) {
  settings = { ...settings, ...patch };
  await invoke("set_settings", { settings });
  toast("设置已保存", 1400);
  if (tab === "settings") render();
}

async function addLink() {
  const input = $("#link");
  const text = input.value.trim();
  if (!text) return input.focus();
  const n = await invoke("add_links", { text });
  if (n === 0) toast("没有识别到下载链接");
  else { input.value = ""; toast(n > 1 ? `已添加 ${n} 个链接` : settings?.ask_before_download ? "已添加，请在弹出的窗口里确认" : "已开始下载"); }
}

function go(next, nextCat = "") {
  tab = next; cat = nextCat;
  if (tab !== "done") query = "";
  $("#tools").innerHTML = "";
  $("#main").scrollTop = 0;
  if (tab === "done") loadHistory(); else render();
}

onAct(document.body, {
  tab: (_, el) => go(el.dataset.tab),
  cat: (_, el) => go("done", el.dataset.cat),
  flt: (_, el) => { flt = el.dataset.flt; render(); },
  add: addLink,
  pause: (id) => invoke("pause", { id }),
  resume: (id) => invoke("resume", { id }),
  cancel: (id) => invoke("cancel", { id }),
  undoAll: () => cancelledTasks().forEach((t) => invoke("undo_cancel", { id: t.id })),
  folder: (id) => invoke("open_folder", { id }),
  pauseAll: () => invoke("pause_all"),
  resumeAll: () => activeTasks().filter((t) => t.status === "paused" || t.status === "failed").forEach((t) => invoke("resume", { id: t.id })),
  openPath: (_, el) => invoke("open_path", { path: history[+el.dataset.i].path, reveal: false }).catch(() => toast("文件已被移动或删除")),
  revealPath: (_, el) => invoke("open_path", { path: history[+el.dataset.i].path, reveal: true }).catch(() => toast("文件已被移动或删除")),
  forget: async (_, el) => { await invoke("remove_history", { path: history[+el.dataset.i].path }); loadHistory(); },
  toggle: (_, el) => save({ [el.dataset.key]: !settings[el.dataset.key] }),
  theme: (_, el) => save({ theme: el.dataset.key }),
  pickBase: async () => { const p = await invoke("pick_folder", { start: settings.base_dir }); if (p) save({ base_dir: p, category_dirs: {} }); },
});
$("#link").addEventListener("keydown", (e) => { if (e.key === "Enter") addLink(); });

async function loadHistory() { history = await invoke("history"); render(); }

(async () => {
  settings = await invoke("get_settings");
  version = await invoke("app_version").catch(() => "");
  tasks = await invoke("tasks");
  history = await invoke("history");
  render();
  renderGauge();
  listen("tasks", (all) => {
    const doneBefore = tasks.filter((t) => t.status === "done").length;
    tasks = all;
    renderGauge();
    if (all.filter((t) => t.status === "done").length !== doneBefore) { loadHistory(); return; }
    if (tab === "active") render(); else { renderNav(); renderUndo(); }
  });
})();
