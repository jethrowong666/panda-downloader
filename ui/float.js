// 右上角悬浮窗：只显示已确认、还没结束的任务。
const W = 316, PILL_W = 200, MAX_ROWS = 4;
let collapsed = false;
$("#colBtn").innerHTML = ICON.minus;
$("#mainBtn").innerHTML = ICON.window;
$("#drop").innerHTML = `${ICON.inbox}<span>松开就开始下载</span>`;

function row(t) {
  const p = pct(t);
  let right, stats, barCls = "", ops = "";
  if (t.status === "cancelled") {
    right = `<button class="link" data-act="undo" data-id="${t.id}">撤销</button>`;
    stats = `<span>已取消</span><span class="dot">${Math.ceil((t.undo_ms || 0) / 1000)} 秒后删除</span>`;
    barCls = "paused";
  } else if (t.status === "failed") {
    right = `<span class="pct" style="color:var(--danger)">失败</span>`;
    stats = `<span class="err ellipsis" title="${esc(t.error || "")}">${esc(errFirst(t.error))}</span>`;
    ops = `<button class="icon" data-act="resume" data-id="${t.id}" title="重试">${ICON.retry}</button>`;
    barCls = "failed";
  } else if (t.status === "paused") {
    right = `<span class="pct num">${t.total ? p.toFixed(1) + "%" : human(t.downloaded)}</span>`;
    stats = `<span>已暂停</span>${t.total ? `<span class="dot num">${human(t.downloaded)} / ${human(t.total)}</span>` : ""}`;
    ops = `<button class="icon" data-act="resume" data-id="${t.id}" title="继续">${ICON.play}</button>`;
    barCls = "paused";
  } else {
    right = `<span class="pct num">${t.probing ? "" : t.total ? p.toFixed(1) + "%" : human(t.downloaded)}</span>`;
    stats = t.video?.merging ? "<span>正在合并音视频…</span>" : t.video?.resolving ? "<span>正在解析视频…</span>" : t.bt?.resolving ? "<span>正在获取种子信息…</span>" : t.probing ? "<span>正在连接…</span>"
      : `<span class="num">${speed(t.speed) || "等待数据…"}</span>${t.total && t.speed ? `<span class="dot num">剩余 ${eta(t)}</span>` : ""}` + (t.bt ? `<span class="dot num">${t.bt.peers} 个用户</span>` + (t.bt.web ? '<span class="dot">网络种子加速</span>' : "") : "");
    ops = `<button class="icon" data-act="pause" data-id="${t.id}" title="暂停">${ICON.pause}</button>`;
    if (!t.total) barCls = "indeterminate";
  }
  if (t.status !== "cancelled") ops += `<button class="icon danger" data-act="cancel" data-id="${t.id}" title="取消">${ICON.x}</button>`;
  return `<div class="task" data-key="f${t.id}">
    <div class="top"><span class="grow ellipsis" title="${esc(t.name)}">${esc(t.name)}</span>${right}<span class="ops">${ops}</span></div>
    <div class="bar ${barCls}"><i style="width:${t.total ? p : 0}%"></i></div>
    <div class="stats">${stats}</div></div>`;
}

function render(all) {
  const list = all.filter((t) => t.confirmed && ["downloading", "paused", "failed", "cancelled"].includes(t.status));
  const total = list.reduce((a, t) => a + (t.status === "downloading" ? t.speed : 0), 0);
  const running = list.filter((t) => t.status === "downloading").length;
  $("#total").textContent = running ? speed(total) || "0 B/s" : "已暂停";
  $("#count").textContent = `${list.length} 个任务`;
  // 需要用户处理的排前面：可撤销的、失败的，然后是正在下载、已暂停。
  const order = { cancelled: 0, failed: 1, downloading: 2, paused: 3 };
  const shown = [...list].sort((a, b) => order[a.status] - order[b.status]).slice(0, MAX_ROWS);
  morph($("#list"), shown.map(row).join("") +
    (list.length > MAX_ROWS ? `<div class="more" data-act="main">还有 ${list.length - MAX_ROWS} 个任务，查看全部</div>` : ""));
  const sized = list.filter((t) => t.total);
  const all_pct = sized.length ? (sized.reduce((a, t) => a + t.downloaded, 0) * 100) / sized.reduce((a, t) => a + t.total, 0) : 0;
  $("#pillText").textContent = `${all_pct.toFixed(0)}%  ${running ? speed(total) || "0 B/s" : "已暂停"}`;
  fit(collapsed ? PILL_W : W);
}

onAct(document.body, {
  pause: (id) => invoke("pause", { id }),
  resume: (id) => invoke("resume", { id }),
  cancel: (id) => invoke("cancel", { id }),
  undo: (id) => invoke("undo_cancel", { id }),
  main: () => invoke("show_main"),
  collapse: () => { collapsed = true; document.body.classList.add("collapsed"); fit(PILL_W); },
  expand: () => { collapsed = false; document.body.classList.remove("collapsed"); fit(W); },
});
$("#head").addEventListener("dblclick", () => invoke("show_main"));

// 把链接拖到悬浮窗上就开始下载。
let dragDepth = 0;
document.addEventListener("dragenter", (e) => { e.preventDefault(); if (dragDepth++ === 0) { document.body.classList.add("dragging"); fit(W); } });
document.addEventListener("dragleave", () => { if (--dragDepth <= 0) { dragDepth = 0; document.body.classList.remove("dragging"); fit(W); } });
document.addEventListener("dragover", (e) => e.preventDefault());
document.addEventListener("drop", async (e) => {
  e.preventDefault();
  dragDepth = 0;
  document.body.classList.remove("dragging");
  const text = e.dataTransfer.getData("text/uri-list") || e.dataTransfer.getData("text/plain");
  if (text && (await invoke("add_links", { text })) === 0) toast("没有识别到下载链接");
});

(async () => {
  render(await invoke("tasks"));
  listen("tasks", render);
})();
