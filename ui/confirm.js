// 确认窗：弹出时文件已经在后台下载，用户只需按回车。
const group = Number(new URLSearchParams(location.search).get("g"));
const W = 480;
let tasks = [];
let mode = "";            // single | batch | dup | unsupported
let editedName = false;
let dir = "";             // 用户选的保存位置（空 = 用任务默认的）
const checked = new Set();
let busy = false;

const body = $("#body"), foot = $("#foot"), pre = $("#pre");
$("#closeX").innerHTML = ICON.x;

const pathBox = (d) => `<div class="path" data-act="pick" title="点击更换保存位置">${ICON.folder}<span class="p" data-path-text="${esc(d)}"></span><span class="change">更改</span></div>`;
const askBox = '<label class="check"><input type="checkbox" id="noask">以后不再询问，直接下载</label>';

function render() {
  const live = tasks.filter((t) => !t.confirmed);
  const dup = live.find((t) => t.dup_of);
  const ok = live.filter((t) => !t.unsupported && !t.dup_of);
  const next = dup ? "dup" : ok.length === 0 && live.length ? "unsupported" : ok.length > 1 ? "batch"
    : ok[0]?.bt ? "bt" : ok[0]?.video ? (ok[0].video.title ? "video" : "video-loading") : "single";
  if (next !== mode) { mode = next; build(live, ok, dup); }
  update(live, ok);
  fit(W);
}

function build(live, ok, dup) {
  if (mode === "dup") {
    $("#title").textContent = "这个文件已经下载过";
    dir = dup.dir;
    body.innerHTML = `
      <div class="file">${fileBadge(dup.category, 36)}
        <div class="grow col"><div class="name ellipsis" style="padding:6px 0 0" title="${esc(dup.name)}">${esc(dup.name)}</div>
        <div class="small muted" style="margin-top:2px">文件还在，位于</div></div></div>
      <div class="path" data-act="reveal" style="margin-top:12px" title="在文件夹中显示">${ICON.folder}<span class="p" data-path-text="${esc(dup.dup_of)}"></span><span class="change">查看</span></div>`;
    foot.innerHTML = `<span class="grow"></span><button data-act="again">再下一份</button><button class="primary" data-act="existing">打开已有的</button>`;
  } else if (mode === "unsupported") {
    $("#title").textContent = "暂不支持这类链接";
    body.innerHTML = live.map((t) => `<div class="notice" style="margin-bottom:8px"><b>${esc(t.unsupported)}</b><div class="clamp2" style="margin-top:2px;opacity:.85">${esc(t.url)}</div></div>`).join("");
    foot.innerHTML = `<span class="grow"></span><button class="primary" data-act="cancel">知道了</button>`;
  } else if (mode === "video-loading") {
    const t = ok[0];
    let host = "";
    try { host = new URL(t.url).hostname.replace(/^www\./, ""); } catch {}
    $("#title").textContent = "下载视频";
    body.innerHTML = `<div class="vid"><div class="thumb loading"></div>
      <div class="grow col" style="padding-top:4px"><div class="row"><span class="spin"></span><span class="vtitle">正在解析视频…</span></div>
      <div class="vmeta">${esc(host)}<span class="dot">通常需要几秒钟</span></div></div></div><div class="err" id="err"></div>`;
    foot.innerHTML = `<span class="grow"></span><button data-act="cancel">取消</button><button class="primary" data-act="wait" id="startBtn">开始下载</button>`;
  } else if (mode === "video") {
    const t = ok[0], v = t.video;
    dir = dir || t.dir;
    $("#title").textContent = "下载视频";
    const meta = [v.site, v.uploader].filter(Boolean).map((s, i) => `<span class="${i ? "dot" : ""}">${esc(s)}</span>`).join("");
    body.innerHTML = `<div class="vid">
        <div class="thumb">${v.thumbnail ? `<img src="${esc(v.thumbnail)}" referrerpolicy="no-referrer" alt="" style="width:100%;height:100%;object-fit:cover">` : ""}
          ${v.duration ? `<span class="dur num">${clock(v.duration)}</span>` : ""}</div>
        <div class="grow col"><div class="vtitle clamp2" title="${esc(v.title)}">${esc(v.title)}</div><div class="vmeta">${meta}</div></div></div>
      <div class="lbl">清晰度</div><div class="qual" id="qual"></div>
      <div class="lbl">文件名</div><input type="text" id="name" value="${esc(t.name)}" spellcheck="false">
      <div class="lbl">保存到</div>${pathBox(dir)}<div class="err" id="err"></div>`;
    foot.innerHTML = `${askBox}<span class="grow"></span><button data-act="cancel">取消</button><button class="primary" data-act="start">开始下载</button>`;
    $("#name").addEventListener("input", () => (editedName = true));
  } else if (mode === "bt") {
    const t = ok[0];
    dir = dir || t.dir;
    $("#title").textContent = "BT 下载";
    body.innerHTML = `<div class="file"><span class="badge">${ICON.magnet}</span>
        <div class="grow col"><div class="name ellipsis" id="btName" style="padding:6px 0 0"></div>
        <div class="meta num" id="meta"></div></div></div>
      <div class="lbl">保存到</div>${pathBox(dir)}<div id="btNote"></div><div class="err" id="err"></div>`;
    foot.innerHTML = `${askBox}<span class="grow"></span><button data-act="cancel">取消</button><button class="primary" data-act="start">开始下载</button>`;
  } else if (mode === "batch") {
    $("#title").textContent = `新建下载 · ${ok.length} 个文件`;
    ok.forEach((t) => checked.add(t.id));
    dir = dir || ok[0].dir;
    body.innerHTML = `<div class="list" id="list"></div><div class="lbl">保存到</div>${pathBox(dir)}`;
    foot.innerHTML = `${askBox}<span class="grow"></span><button data-act="cancel">取消</button><button class="primary" data-act="start" id="startBtn">全部开始</button>`;
    $("#list").addEventListener("change", (e) => {
      if (e.target.type === "checkbox") {
        e.target.checked ? checked.add(+e.target.value) : checked.delete(+e.target.value);
        update(tasks.filter((t) => !t.confirmed), tasks.filter((t) => !t.confirmed && !t.unsupported && !t.dup_of));
      }
    });
  } else {
    $("#title").textContent = "新建下载";
    const t = ok[0];
    dir = dir || t.dir;
    body.innerHTML = `
      <div class="file"><span class="badge" id="badge">${fileBadge(t.category, 36)}</span>
        <div class="grow col"><input type="text" class="fname" id="name" value="${esc(t.name)}" spellcheck="false" title="点击修改文件名">
        <div class="meta num" id="meta"></div></div></div>
      <div class="lbl">保存到</div>${pathBox(dir)}<div class="err" id="err"></div>`;
    foot.innerHTML = `${askBox}<span class="grow"></span><button data-act="cancel">取消</button><button class="primary" data-act="start">开始下载</button>`;
    const n = $("#name");
    n.addEventListener("input", () => (editedName = true));
    n.addEventListener("focus", () => { const i = n.value.lastIndexOf("."); n.setSelectionRange(0, i > 0 ? i : n.value.length); }, { once: true });
  }
  requestAnimationFrame(() => renderPaths(body));
}

function update(live, ok) {
  if (mode === "single" && ok[0]) {
    const t = ok[0];
    const n = $("#name");
    if (!editedName && document.activeElement !== n && n.value !== t.name) n.value = t.name;
    const bits = [t.total != null ? human(t.total) : t.probing ? "正在获取大小…" : "大小未知", t.url.startsWith("https") ? "HTTPS" : "HTTP"];
    if (t.free_space != null) bits.push(`剩余空间 ${human(t.free_space)}`);
    $("#meta").innerHTML = bits.map((b, i) => `<span class="${i ? "dot" : ""}">${b}</span>`).join("") + (t.resumable ? '<span class="chip">可断点续传</span>' : "");
    const err = $("#err");
    if (t.total != null && t.free_space != null && t.total - t.downloaded > t.free_space) err.innerHTML = '<div class="notice error">磁盘空间不够，换个保存位置吧</div>';
    else if (t.status === "failed" && t.error) err.innerHTML = `<div class="notice error">${errHtml(t.error)}</div>`;
    else err.innerHTML = "";
    setPre(t.downloaded, t.status);
  } else if (mode === "bt") {
    const t = ok[0], b = t.bt;
    const nm = $("#btName");
    nm.innerHTML = b.resolving ? '<span class="row"><span class="spin"></span>正在获取种子信息…</span>' : esc(t.name);
    nm.title = b.resolving ? "" : t.name;
    const bits = [];
    if (t.total != null) bits.push(human(t.total));
    else if (b.resolving) bits.push("磁力链接要先从网络上找到种子，一般十几秒");
    if (b.files > 1) bits.push(`${b.files} 个文件`);
    if (!b.resolving) bits.push(b.peers ? `已连上 ${b.peers} 个用户` : "正在找用户…");
    if (t.free_space != null) bits.push(`剩余空间 ${human(t.free_space)}`);
    $("#meta").innerHTML = bits.map((x, i) => `<span class="${i ? "dot" : ""}">${x}</span>`).join("") + '<span class="chip">可断点续传</span>' + (b.private ? '<span class="chip">私有种子</span>' : "");
    $("#btNote").innerHTML = b.private ? '<div class="small muted" style="margin-top:8px">私有种子（PT）：只连种子里的服务器，不用 DHT 和用户交换，符合 PT 站规则。</div>' : "";
    const err = $("#err");
    if (t.status === "failed" && t.error) err.innerHTML = `<div class="notice error">${errHtml(t.error)}</div>`;
    else if (t.total != null && t.free_space != null && t.total - t.downloaded > t.free_space) err.innerHTML = '<div class="notice error">磁盘空间不够，换个保存位置吧</div>';
    else err.innerHTML = "";
    setPre(t.downloaded, t.status);
  } else if (mode === "video-loading") {
    const t = ok[0];
    if (t.status === "failed" && t.error) {
      $("#err").innerHTML = `<div class="notice error">${errHtml(t.error)}</div>`;
      $("#startBtn").textContent = "知道了";
      $("#startBtn").dataset.act = "cancel";
      body.querySelector(".spin")?.remove();
      body.querySelector(".vmeta .dot")?.remove();
      foot.querySelector('[data-act="cancel"]:not(.primary)')?.remove();
      body.querySelector(".vtitle").textContent = "没能解析这个视频";
      body.querySelector(".thumb").classList.remove("loading");
    }
  } else if (mode === "video") {
    const t = ok[0], v = t.video;
    morph($("#qual"), v.choices.map((c, i) => `<div class="q ${c.key === v.choice ? "on" : ""}" data-act="qual" data-key="${esc(c.key)}">
      <span class="dotr"></span><span class="ql">${esc(c.label)}</span><span class="qd">${esc(c.detail)}</span>${i === 0 ? '<span class="best">最佳</span>' : ""}
      <span class="qs num">${c.size ? "约 " + human(c.size) : ""}</span></div>`).join(""));
    const n = $("#name");
    if (!editedName && document.activeElement !== n && n.value !== t.name) n.value = t.name;
    const err = $("#err");
    if (t.status === "failed" && t.error) err.innerHTML = `<div class="notice error">${errHtml(t.error)}</div>`;
    else if (t.total != null && t.free_space != null && t.total - t.downloaded > t.free_space) err.innerHTML = '<div class="notice error">磁盘空间不够，换个保存位置吧</div>';
    else err.innerHTML = "";
    setPre(t.downloaded, t.status);
  } else if (mode === "batch") {
    morph($("#list"), ok.map((t) => `<label class="item" data-key="c${t.id}"><input type="checkbox" value="${t.id}" ${checked.has(t.id) ? "checked" : ""}>
      ${fileBadge(t.category, 20)}<span class="grow ellipsis" title="${esc(t.name)}">${esc(t.name)}</span><span class="sz num">${t.total != null ? human(t.total) : "…"}</span></label>`).join(""));
    const total = ok.filter((t) => checked.has(t.id)).reduce((a, t) => a + (t.total || 0), 0);
    $("#startBtn").textContent = checked.size === ok.length ? `全部开始` : `开始 ${checked.size} 个`;
    $("#startBtn").title = total ? `共 ${human(total)}` : "";
    setPre(ok.reduce((a, t) => a + t.downloaded, 0), "downloading");
  } else pre.hidden = true;
}

function setPre(n, status) {
  pre.hidden = !(n > 0) || status === "failed";
  pre.textContent = status === "done" ? "已经下载好了" : `已在后台下载 ${human(n)}`;
}

async function applyNoAsk() {
  if ($("#noask")?.checked) {
    const s = await invoke("get_settings");
    await invoke("set_settings", { settings: { ...s, ask_before_download: false } });
  }
}

async function start() {
  if (busy) return;
  const live = tasks.filter((t) => !t.confirmed && !t.unsupported && !t.dup_of);
  let items;
  if (mode === "batch") items = live.filter((t) => checked.has(t.id)).map((t) => ({ id: t.id, name: t.name, dir }));
  else if (mode === "bt") items = [{ id: live[0].id, name: live[0].name, dir }];
  else {
    const name = ($("#name")?.value || "").trim();
    if (!name) { $("#name").focus(); return; }
    items = [{ id: live[0].id, name, dir }];
  }
  if (!items.length) return cancel();
  busy = true;
  await applyNoAsk();
  await invoke("confirm_group", { group, items });
}
async function cancel() { if (!busy) { busy = true; await invoke("cancel_group", { group }); } }

function clock(sec) {
  const s = Math.round(sec), h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), r = s % 60;
  return h ? `${h}:${String(m).padStart(2, "0")}:${String(r).padStart(2, "0")}` : `${m}:${String(r).padStart(2, "0")}`;
}

onAct(document.body, {
  start,
  cancel,
  wait: () => toast("还在解析视频，稍等一下"),
  qual: (_, el) => {
    const t = tasks.find((x) => x.video && !x.confirmed);
    if (!t || el.classList.contains("on")) return;
    editedName = false;
    body.querySelectorAll(".q").forEach((q) => q.classList.toggle("on", q === el));
    invoke("set_video_choice", { id: t.id, key: el.dataset.key });
  },
  existing: () => invoke("open_existing", { group }),
  reveal: () => { const t = tasks.find((x) => x.dup_of); if (t) invoke("open_path", { path: t.dup_of, reveal: true }); },
  again: async () => {
    const t = tasks.find((x) => x.dup_of);
    busy = true;
    await invoke("confirm_group", { group, items: [{ id: t.id, name: t.name, dir: t.dir }] });
  },
  pick: async () => {
    const p = await invoke("pick_folder", { start: dir });
    if (p) {
      dir = p;
      const box = body.querySelector(".path .p");
      box.dataset.pathText = p;
      renderPaths(body);
    }
  },
});

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") { e.preventDefault(); cancel(); }
  if (e.key === "Enter" && !e.target.closest("button")) {
    e.preventDefault();
    if (mode === "dup") invoke("open_existing", { group });
    else if (mode === "unsupported") cancel();
    else if (mode === "video-loading") toast("还在解析视频，稍等一下");
    else start();
  }
});

(async () => {
  tasks = await invoke("group_tasks", { group });
  render();
  listen("tasks", (all) => { const mine = all.filter((t) => t.group === group); if (mine.length) { tasks = mine; render(); } });
})();
