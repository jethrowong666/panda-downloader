// 主窗口：侧边栏 + 正在下载 / 已完成 / 设置。
let tab = "active";
let tasks = [];
let history = [];
let settings = null;

$("#linkIcon").innerHTML = ICON.link;
$("#addBtn").innerHTML = `${ICON.plus}新建下载`;

const TITLES = { active: "正在下载", done: "已完成", settings: "设置" };

function renderNav() {
  const active = tasks.filter((t) => t.confirmed && t.status !== "done");
  $("#navActive").innerHTML = `${ICON.inbox}<span>正在下载</span><span class="n num">${active.length || ""}</span>`;
  $("#navDone").innerHTML = `${ICON.done}<span>已完成</span><span class="n num">${history.length || ""}</span>`;
  $("#navSet").innerHTML = `${ICON.gear}<span>设置</span>`;
  document.querySelectorAll(".nav").forEach((n) => n.classList.toggle("on", n.dataset.tab === tab));
  const sp = active.reduce((a, t) => a + (t.status === "downloading" ? t.speed : 0), 0);
  $("#sideSpeed").textContent = speed(sp) || "0 B/s";
  $("#pageTitle").textContent = TITLES[tab];
  $("#addBox").hidden = tab === "settings";
}

function activeRow(t) {
  const p = pct(t);
  let sub, barCls = "", left = "", right = "";
  switch (t.status) {
    case "failed":
      sub = `<span class="err" title="${esc(t.error || "")}">${esc(errFirst(t.error))}</span>`; barCls = "failed"; left = "失败"; break;
    case "cancelled":
      sub = `<span>已取消</span><span class="dot">${Math.ceil((t.undo_ms || 0) / 1000)} 秒后删除</span>`; barCls = "paused"; break;
    case "paused":
      sub = `<span class="num">${human(t.downloaded)}${t.total ? " / " + human(t.total) : ""}</span><span class="dot">已暂停</span>`; barCls = "paused"; break;
    default:
      sub = t.video?.merging ? "<span>正在合并音视频…</span>" : t.video?.resolving ? "<span>正在解析视频…</span>" : t.probing ? "<span>正在连接…</span>"
        : `<span class="num">${human(t.downloaded)}${t.total ? " / " + human(t.total) : ""}</span>${t.speed ? `<span class="dot num">${speed(t.speed)}</span>` : ""}${t.total && t.speed ? `<span class="dot num">剩余 ${eta(t)}</span>` : ""}`;
      if (!t.total) barCls = "indeterminate";
  }
  if (!left) left = t.total ? `${p.toFixed(1)}%` : "";
  right = t.resumable ? "可续传" : "";
  const ops = [];
  if (t.status === "downloading") ops.push(`<button class="icon" data-act="pause" data-id="${t.id}" title="暂停">${ICON.pause}</button>`);
  if (t.status === "paused") ops.push(`<button class="icon" data-act="resume" data-id="${t.id}" title="继续">${ICON.play}</button>`);
  if (t.status === "failed") ops.push(`<button class="icon" data-act="resume" data-id="${t.id}" title="重试">${ICON.retry}</button>`);
  if (t.status === "cancelled") ops.push(`<button class="link" data-act="undo" data-id="${t.id}">撤销</button>`);
  else {
    ops.push(`<button class="icon" data-act="folder" data-id="${t.id}" title="打开所在文件夹">${ICON.folder}</button>`);
    ops.push(`<button class="icon danger" data-act="cancel" data-id="${t.id}" title="取消">${ICON.x}</button>`);
  }
  return `<div class="t" data-key="a${t.id}">${fileBadge(t.category, 32)}
    <div class="info"><div class="name ellipsis" title="${esc(t.name)}">${esc(t.name)}</div><div class="sub">${sub}</div></div>
    <div class="prog"><div class="bar ${barCls}"><i style="width:${t.total ? p : 0}%"></i></div><div class="l num"><span>${left}</span><span>${right}</span></div></div>
    <div class="ops ${t.status === "cancelled" ? "show" : ""}">${ops.join("")}</div></div>`;
}

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

function doneRow(h, i) {
  const name = h.path.split(/[\\/]/).pop();
  const folder = h.path.slice(0, h.path.length - name.length - 1);
  return `<div class="t h" data-key="${esc(h.path)}">${fileBadge(catOf(name), 32)}
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
  const main = $("#main"), tools = $("#tools");
  if (tab === "active") {
    const list = tasks.filter((t) => t.confirmed && t.status !== "done");
    const running = list.filter((t) => t.status === "downloading").length;
    const paused = list.filter((t) => t.status === "paused" || t.status === "failed").length;
    morph(tools, list.length
      ? `<span class="sum num">${list.length} 个任务${running ? `，${running} 个下载中` : ""}</span><span class="grow"></span>
         ${paused ? `<button data-act="resumeAll">${ICON.play}全部开始</button>` : ""}${running ? `<button data-act="pauseAll">${ICON.pause}全部暂停</button>` : ""}`
      : "");
    morph(main, list.length ? `<div class="list">${list.map(activeRow).join("")}</div>`
      : empty("没有正在下载的任务", ["复制一个下载链接，确认窗口会自动弹出", "也可以按 <kbd>Ctrl</kbd> <kbd>Alt</kbd> <kbd>D</kbd> 下载剪贴板里的链接"]));
  } else if (tab === "done") {
    tools.innerHTML = history.length ? `<span class="sum num">共 ${history.length} 个文件</span>` : "";
    main.innerHTML = history.length ? `<div class="list">${history.map(doneRow).join("")}</div>` : empty("还没有下载完成的文件", ["下载完成的文件会出现在这里"]);
    requestAnimationFrame(() => renderPaths(main));
  } else {
    tools.innerHTML = "";
    renderSettings(main);
  }
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
      ${row("全局限速", "所有任务加起来的最高速度，0 表示不限速", `<input type="number" id="limit" min="0" step="0.5" value="${+(s.speed_limit / 1048576).toFixed(1)}" style="width:88px"><span class="unit">MB/s</span>`)}
    </div></div>
    <div class="group"><h3>使用习惯</h3><div class="box">
      ${row("下载前确认", "关闭后复制链接就直接开始下载，不再弹出确认窗口", sw("ask_before_download"))}
      ${row("自动识别复制的链接", "复制下载链接时自动弹出；复制普通网页地址不会打扰你", sw("watch_clipboard"))}
      ${row("启动后继续未完成的任务", "电脑重启或软件退出后，下次打开自动从断点接着下载", sw("auto_resume"))}
    </div></div>
    <div class="group"><h3>快捷键</h3><div class="box">
      ${row("下载剪贴板里的链接", "任何时候都可以使用", "<span><kbd>Ctrl</kbd> <kbd>Alt</kbd> <kbd>D</kbd></span>")}
    </div></div>
    <div class="about">熊猫下载器 0.2.6 · 开源软件（GPL-3.0）</div>`;
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
  else { input.value = ""; toast(n > 1 ? `已添加 ${n} 个链接` : "已添加，请在弹出的窗口里确认"); }
}

onAct(document.body, {
  tab: (_, el) => { tab = el.dataset.tab; if (tab === "done") loadHistory(); $("#main").scrollTop = 0; render(); },
  add: addLink,
  pause: (id) => invoke("pause", { id }),
  resume: (id) => invoke("resume", { id }),
  cancel: (id) => invoke("cancel", { id }),
  undo: (id) => invoke("undo_cancel", { id }),
  folder: (id) => invoke("open_folder", { id }),
  pauseAll: () => invoke("pause_all"),
  resumeAll: () => tasks.filter((t) => t.confirmed && (t.status === "paused" || t.status === "failed")).forEach((t) => invoke("resume", { id: t.id })),
  openPath: (_, el) => invoke("open_path", { path: history[+el.dataset.i].path, reveal: false }).catch(() => toast("文件已被移动或删除")),
  revealPath: (_, el) => invoke("open_path", { path: history[+el.dataset.i].path, reveal: true }).catch(() => toast("文件已被移动或删除")),
  forget: async (_, el) => { await invoke("remove_history", { path: history[+el.dataset.i].path }); loadHistory(); },
  toggle: (_, el) => save({ [el.dataset.key]: !settings[el.dataset.key] }),
  pickBase: async () => { const p = await invoke("pick_folder", { start: settings.base_dir }); if (p) save({ base_dir: p, category_dirs: {} }); },
});
$("#link").addEventListener("keydown", (e) => { if (e.key === "Enter") addLink(); });

async function loadHistory() { history = await invoke("history"); render(); }

(async () => {
  settings = await invoke("get_settings");
  tasks = await invoke("tasks");
  history = await invoke("history");
  render();
  listen("tasks", (all) => {
    const doneBefore = tasks.filter((t) => t.status === "done").length;
    tasks = all;
    if (all.filter((t) => t.status === "done").length !== doneBefore) { loadHistory(); return; }
    if (tab === "active") render(); else renderNav();
  });
})();
