// 右上角悬浮窗：只显示已确认、还没结束的任务。
const W = 336, PILL_W = 172, MAX_ROWS = 4;
// 默认是小窗口，只显示总速度；点开才看每个任务
let collapsed = true;
$("#colBtn").innerHTML = ICON.minus;
$("#mainBtn").innerHTML = ICON.window;
$("#drop").innerHTML = `${ICON.inbox}<span>松开就开始下载</span>`;

function row(t) {
  const p = pct(t);
  let right, stats, barCls = "", ops = "";
  if (t.status === "failed") {
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
      : `<span class="num">${speed(t.speed) || "等待数据…"}</span>${t.total && t.speed ? `<span class="dot num">剩余 ${eta(t)}</span>` : ""}` + (t.bt ? (t.bt.web ? '<span class="dot" title="正在用种子附带的 HTTP 地址加速">HTTP 加速</span>' : `<span class="dot num" title="已连上的 BT 用户">${t.bt.peers} 人</span>`) : "");
    ops = `<button class="icon" data-act="pause" data-id="${t.id}" title="暂停">${ICON.pause}</button>`;
    if (!t.total) barCls = "indeterminate";
  }
  ops += `<button class="icon danger" data-act="cancel" data-id="${t.id}" title="删除任务和已下载的数据（可撤销）">${ICON.trash}</button>`;
  return `<div class="task" data-key="f${t.id}">
    <div class="top"><span class="grow ellipsis" title="${esc(t.name)}">${esc(t.name)}</span>${right}<span class="ops">${ops}</span></div>
    <div class="bar ${barCls}"><i style="width:${t.total ? p : 0}%"></i></div>
    <div class="stats">${stats}</div></div>`;
}

function render(all) {
  const gone = all.filter((t) => t.confirmed && t.status === "cancelled");
  const list = all.filter((t) => t.confirmed && ["downloading", "paused", "failed"].includes(t.status));
  const total = list.reduce((a, t) => a + (t.status === "downloading" ? t.speed : 0), 0);
  const running = list.filter((t) => t.status === "downloading").length;
  $("#total").textContent = running ? speed(total) || "0 B/s" : "已暂停";
  $("#count").textContent = `${list.length} 个任务`;
  // 需要用户处理的排前面：失败的，然后是正在下载、已暂停。
  const order = { failed: 1, downloading: 2, paused: 3 };
  const shown = [...list].sort((a, b) => order[a.status] - order[b.status]).slice(0, MAX_ROWS);
  // 刚删除的任务：不占一行，只留一条可以撤销的提示
  const goneBar = gone.length ? `<div class="gone" data-key="gone"><span>${gone.length === 1 ? `已删除「${esc(gone[0].name)}」` : `已删除 ${gone.length} 个任务`}</span><button data-act="undoAll">撤销</button>
    <i style="width:${Math.min(100, (Math.max(...gone.map((t) => t.undo_ms || 0)) / Math.max(...gone.map((t) => t.undo_total_ms || 5000))) * 100)}%"></i></div>` : "";
  morph($("#list"), goneBar + shown.map(row).join("") +
    (list.length > MAX_ROWS ? `<div class="more" data-act="main">还有 ${list.length - MAX_ROWS} 个任务，查看全部</div>` : ""));
  const sized = list.filter((t) => t.total);
  const all_pct = sized.length ? (sized.reduce((a, t) => a + t.downloaded, 0) * 100) / sized.reduce((a, t) => a + t.total, 0) : 0;
  const failed = list.filter((t) => t.status === "failed").length;
  const [sv, su] = (speed(total) || "0 B/s").split(" ");
  $("#pillText").innerHTML = running ? `${sv}<i>${su}</i>` : failed ? "下载失败" : list.length ? "已暂停" : "已删除";
  $("#pillCount").textContent = failed ? `${failed} 个失败` : `${list.length}`;
  $("#pillCount").title = failed ? "点开查看原因" : `${list.length} 个任务`;
  $("#pill").classList.toggle("warn", failed > 0);
  $("#pillArc").style.strokeDashoffset = (87.96 * (1 - all_pct / 100)).toFixed(2);
  $("#pill").title = `总进度 ${all_pct.toFixed(0)}%，点开查看每个任务`;
  fit(collapsed ? PILL_W : W);
}

onAct(document.body, {
  pause: (id) => invoke("pause", { id }),
  resume: (id) => invoke("resume", { id }),
  cancel: (id) => invoke("cancel", { id }),
  undoAll: async () => (await invoke("tasks")).filter((t) => t.status === "cancelled").forEach((t) => invoke("undo_cancel", { id: t.id })),
  main: () => invoke("show_main"),
  collapse: () => { collapsed = true; document.body.classList.add("collapsed"); fit(PILL_W); },
  expand: () => { collapsed = false; document.body.classList.remove("collapsed"); fit(W); },
});
$("#head").addEventListener("dblclick", () => invoke("show_main"));

// 把链接拖到悬浮窗上就开始下载。
let dragDepth = 0;
document.addEventListener("dragenter", (e) => { e.preventDefault(); if (dragDepth++ === 0) { document.body.classList.add("dragging"); fit(W); } });
document.addEventListener("dragleave", () => { if (--dragDepth <= 0) { dragDepth = 0; document.body.classList.remove("dragging"); fit(collapsed ? PILL_W : W); } });
document.addEventListener("dragover", (e) => e.preventDefault());
document.addEventListener("drop", async (e) => {
  e.preventDefault();
  dragDepth = 0;
  document.body.classList.remove("dragging");
  fit(collapsed ? PILL_W : W);
  const text = e.dataTransfer.getData("text/uri-list") || e.dataTransfer.getData("text/plain");
  if (text && (await invoke("add_links", { text })) === 0) toast("没有识别到下载链接");
});

(async () => {
  render(await invoke("tasks"));
  listen("tasks", render);
})();
