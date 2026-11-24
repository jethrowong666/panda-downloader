const $ = (s) => document.querySelector(s);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
function human(n) {
  if (!n) return "大小未知";
  const u = ["B", "KB", "MB", "GB"]; let v = n, i = 0;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(i ? 1 : 0)} ${u[i]}`;
}
function shortUrl(u) {
  try { const x = new URL(u); const name = decodeURIComponent(x.pathname.split("/").pop() || x.hostname); return name.length > 42 ? name.slice(0, 20) + "…" + name.slice(-18) : name; }
  catch { return u; }
}
let state = null;

async function load() {
  state = await chrome.runtime.sendMessage({ type: "state" });
  const st = $("#st");
  st.className = "st " + (state.connected ? "on" : "off");
  st.querySelector("span").textContent = state.connected ? "已连接" : "未连接";
  $("#ver").textContent = state.version ? `版本 ${state.version}` : "";
  $("#warn").innerHTML = state.connected ? "" : `<div class="warn">连不上熊猫下载器：${esc(state.error || "")}<br>请确认已安装熊猫下载器。</div>`;
  $("#take").classList.toggle("on", !!state.takeover);

  const box = $("#media");
  let html = "";
  if (state.tab?.videoSite) {
    html += `<button class="primary big" data-act="page">下载本页视频<span style="opacity:.7;font-weight:400">· 自动解析清晰度</span></button><div style="height:10px"></div>`;
  }
  if (state.media.length) {
    html += state.media.slice(0, 8).map((m, i) => `<div class="item"><span class="tag">${esc(m.kind)}</span>
      <div class="info"><div class="n" title="${esc(m.url)}">${esc(shortUrl(m.url))}</div><div class="s">${human(m.size)}</div></div>
      <button data-act="one" data-i="${i}">下载</button></div>`).join("");
  } else if (!state.tab?.videoSite) {
    html += `<div class="empty">这个页面还没有发现视频<br>播放一下视频，这里就会出现</div>`;
  }
  box.innerHTML = html;
}

document.addEventListener("click", async (e) => {
  const el = e.target.closest("[data-act]");
  if (!el) return;
  let r;
  if (el.dataset.act === "page") r = await chrome.runtime.sendMessage({ type: "tabVideo" });
  if (el.dataset.act === "one") {
    const m = state.media[+el.dataset.i];
    r = await chrome.runtime.sendMessage({ type: "download", items: [{ url: m.url, video: m.kind === "HLS" || m.kind === "DASH" }], referer: state.tab?.url });
  }
  $("#toast").textContent = r?.ok ? "已交给熊猫下载器，请在弹出的窗口里确认" : `没有成功：${r?.error || "未知原因"}`;
});

$("#take").addEventListener("click", async () => {
  const v = !$("#take").classList.contains("on");
  $("#take").classList.toggle("on", v);
  await chrome.storage.local.set({ takeover: v });
});
$("#open").addEventListener("click", () => chrome.runtime.sendMessage({ type: "show" }));

load();
