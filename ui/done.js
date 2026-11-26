// 右下角完成窗：主按钮按文件类型变化（播放、解压、扫描并运行……），10 秒后自动收起。
const W = 372, LIFETIME = 10000;
let items = [];
let shownAt = Date.now();
let hovering = false;
let busy = false;

$("#ok").innerHTML = ICON.check;
$("#closeBtn").innerHTML = ICON.x;

const SHORT = { extract: "解压", play: "播放", install: "运行", view: "查看", open: "打开" };

function render(all) {
  const next = all.filter((t) => t.confirmed && t.status === "done" && !t.dismissed);
  if (next.some((t) => !items.find((x) => x.id === t.id))) shownAt = Date.now();
  const changed = next.map((t) => t.id).join() !== items.map((t) => t.id).join();
  items = next;
  if (!items.length || !changed) return;
  if (items.length === 1) {
    const t = items[0];
    $("#title").textContent = "下载完成";
    $("#content").innerHTML = `
      <div class="one">${badgeOf(t, 44)}
        <div class="grow col"><div class="name ellipsis" title="${esc(t.name)}">${esc(t.name)}</div>
        <div class="small muted num">${human(t.total ?? t.downloaded)}<span class="dot">${t.bt && t.bt.files > 1 ? `文件夹 · ${t.bt.files} 个文件` : esc(CAT[t.category]?.[1] || "文件")}</span></div></div></div>
      <div class="acts"><button class="primary" data-act="primary" data-id="${t.id}">${esc(t.action_label)}</button>
        <button data-act="folder" data-id="${t.id}">${ICON.folder}打开文件夹</button></div>
      <div class="hint">按 Enter ${esc(t.action_label)}，10 秒后自动收起</div>`;
  } else {
    $("#title").textContent = `${items.length} 个文件已下载完成`;
    $("#content").innerHTML = `<div class="many">${items.slice(0, 5).map((t) => `<div class="item">${badgeOf(t, 26)}
      <span class="grow ellipsis" title="${esc(t.name)}">${esc(t.name)}</span>
      <button class="link" data-act="primary" data-id="${t.id}">${SHORT[t.action] || "打开"}</button>
      <button class="icon" data-act="folder" data-id="${t.id}" title="打开文件夹">${ICON.folder}</button></div>`).join("")}</div>
      ${items.length > 5 ? `<div class="hint" style="padding-top:10px">还有 ${items.length - 5} 个，在主窗口「已完成」里查看</div>` : '<div style="height:4px"></div>'}`;
  }
  fit(W);
}

function dismiss(ids = items.map((t) => t.id)) { invoke("dismiss_done", { ids }); }

async function primary(id) {
  if (busy) return;
  busy = true;
  const t = items.find((x) => x.id === id);
  if (t?.action === "install") toast("正在用 Windows Defender 扫描…", 10000);
  if (t?.action === "extract") toast("正在解压…", 10000);
  try {
    const msg = await invoke("primary_action", { id });
    if (msg) toast(msg);
    setTimeout(() => dismiss([id]), msg ? 1200 : 0);
  } catch (e) {
    toast(String(e), 5000);
  } finally { busy = false; }
}

onAct(document.body, {
  close: () => dismiss(),
  primary,
  folder: (id) => { invoke("open_folder", { id }); dismiss([id]); },
});
document.addEventListener("mouseenter", () => { hovering = true; document.body.classList.add("hover"); });
document.addEventListener("mouseleave", () => { hovering = false; shownAt = Date.now(); document.body.classList.remove("hover"); });
document.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && items.length === 1) primary(items[0].id);
  if (e.key === "Escape") dismiss();
});

// 倒计时：鼠标悬停时暂停。
setInterval(() => {
  if (hovering || busy) return;
  const left = Math.max(0, 1 - (Date.now() - shownAt) / LIFETIME);
  $("#timer").style.transform = `scaleX(${left})`;
  if (left === 0 && items.length) dismiss();
}, 100);

(async () => {
  render(await invoke("tasks"));
  listen("tasks", render);
})();
