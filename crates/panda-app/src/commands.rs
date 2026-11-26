//! 前端可以调用的命令。

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;

use crate::manager::{ConfirmItem, Manager, TaskView};
use crate::store::{HistoryItem, Settings};
use crate::{sys, windows};

type M<'a> = State<'a, Arc<Manager>>;

#[tauri::command]
pub fn tasks(m: M) -> Vec<TaskView> {
    m.views()
}

#[tauri::command]
pub fn group_tasks(m: M, group: u64) -> Vec<TaskView> {
    m.group_views(group)
}

#[tauri::command]
pub async fn confirm_group(m: M<'_>, window: WebviewWindow, group: u64, items: Vec<ConfirmItem>) -> Result<(), String> {
    let _ = window.close();
    m.inner().clone().confirm(group, items).await;
    Ok(())
}

#[tauri::command]
pub async fn cancel_group(m: M<'_>, window: WebviewWindow, group: u64) -> Result<(), String> {
    let _ = window.close();
    m.inner().clone().cancel_group(group).await;
    Ok(())
}

#[tauri::command]
pub fn open_existing(m: M, window: WebviewWindow, group: u64) {
    let _ = window.close();
    m.open_existing(group);
}

#[tauri::command]
pub fn pause(m: M, id: u64) {
    m.pause(id);
}

#[tauri::command]
pub fn resume(m: M, id: u64) {
    m.inner().resume(id);
}

#[tauri::command]
pub fn pause_all(m: M) {
    m.pause_all();
}

#[tauri::command]
pub fn cancel(m: M, id: u64) {
    m.cancel(id);
}

#[tauri::command]
pub fn undo_cancel(m: M, id: u64) {
    m.inner().undo_cancel(id);
}

#[tauri::command]
pub fn remove(m: M, id: u64) {
    m.remove(id);
}

#[tauri::command]
pub fn open_file(m: M, id: u64) -> Result<(), String> {
    m.open_file(id)
}

#[tauri::command]
pub fn open_folder(m: M, id: u64) -> Result<(), String> {
    m.open_folder(id)
}

#[tauri::command]
pub async fn primary_action(m: M<'_>, id: u64) -> Result<Option<String>, String> {
    m.primary_action(id).await
}

#[tauri::command]
pub fn dismiss_done(m: M, ids: Vec<u64>) {
    m.dismiss_done(ids);
}

/// 粘贴或拖入的文字：提取其中所有链接。返回识别到的数量。
#[tauri::command]
pub fn add_links(m: M, text: String) -> usize {
    let mut links = panda_link::extract_links(&text);
    if links.is_empty() && panda_link::parse(text.trim()).is_ok_and(|p| p.engine() != panda_link::Engine::None) {
        links.push(text.trim().to_string());
    }
    let n = links.len();
    if n > 0 {
        m.inner().capture(links);
    }
    n
}

#[tauri::command]
pub fn get_settings(m: M) -> Settings {
    m.settings()
}

#[tauri::command]
pub fn set_settings(m: M, settings: Settings) {
    m.set_settings(settings);
}

#[tauri::command]
pub fn history(m: M) -> Vec<HistoryItem> {
    m.history()
}

#[tauri::command]
pub fn set_video_choice(m: M, id: u64, key: String) {
    m.inner().set_video_choice(id, key);
}

#[tauri::command]
pub fn remove_history(m: M, path: String) {
    m.remove_history(&path);
}

#[tauri::command]
pub fn open_path(path: String, reveal: bool) -> Result<(), String> {
    let p = PathBuf::from(path);
    if reveal {
        sys::reveal(&p)
    } else {
        sys::shell_open(&p)
    }
}

#[tauri::command]
pub async fn pick_folder(app: AppHandle, start: Option<String>) -> Option<String> {
    let mut d = app.dialog().file().set_title("选择保存位置");
    if let Some(s) = start.filter(|s| std::path::Path::new(s).exists()) {
        d = d.set_directory(s);
    }
    d.blocking_pick_folder().and_then(|p| p.into_path().ok()).map(|p| p.display().to_string())
}

/// 前端按内容高度调整窗口大小。
#[tauri::command]
pub fn fit_window(app: AppHandle, window: WebviewWindow, width: f64, height: f64) {
    if window.label() == "done" {
        windows::anchor_done_bottom(&app, height);
    } else if window.label() == "float" {
        // 悬浮窗贴着右边：宽度变化时保持右边缘不动。
        if let (Ok(pos), Ok(size), Ok(scale)) = (window.outer_position(), window.inner_size(), window.scale_factor()) {
            let old_w = size.width as f64 / scale;
            let right = pos.x as f64 / scale + old_w;
            let _ = window.set_size(tauri::LogicalSize::new(width, height));
            if (old_w - width).abs() > 0.5 {
                let _ = window.set_position(tauri::LogicalPosition::new(right - width, pos.y as f64 / scale));
            }
            // 展开后变宽、变高，别跑出屏幕
            windows::keep_float_inside(&app);
        }
    } else {
        let _ = window.set_size(tauri::LogicalSize::new(width, height));
    }
}

#[tauri::command]
pub fn show_main(app: AppHandle) {
    windows::show_main(&app);
}

#[derive(serde::Serialize)]
pub struct BrowserInfo {
    id: &'static str,
    name: &'static str,
}

#[derive(serde::Serialize)]
pub struct ExtInfo {
    /// 扩展文件夹（安装向导要用户在浏览器里选它）。
    dir: Option<String>,
    browsers: Vec<BrowserInfo>,
    /// 扩展最近一次连上来的时间（Unix 秒）：这次运行里的，或者以前记下的。
    seen: Option<u64>,
    /// 这次运行里扩展连上来过（= 现在就能用）。
    connected: bool,
    now: u64,
    hint_off: bool,
}

/// 浏览器扩展的安装情况。
#[tauri::command]
pub fn ext_info(m: M) -> ExtInfo {
    let live = crate::bridge::extension_seen();
    let s = m.settings();
    ExtInfo {
        dir: sys::extension_dir().map(|p| p.display().to_string()),
        browsers: sys::browsers().into_iter().map(|(id, name, _, _)| BrowserInfo { id, name }).collect(),
        seen: live.or(Some(s.ext_seen).filter(|t| *t > 0)),
        connected: live.is_some(),
        now: crate::store::now_secs(),
        hint_off: s.ext_hint_off,
    }
}

/// 安装向导的第一步：打开浏览器，并把"扩展管理页"的地址放进剪贴板。
/// 浏览器不接受从外面直接打开这个内部页面（只会开一个新标签页），所以让用户在地址栏粘贴。返回这个地址。
#[tauri::command]
pub fn ext_install(browser: String) -> Result<String, String> {
    sys::extension_dir().ok_or("没找到扩展文件夹，请重新安装熊猫下载器")?;
    let (_, name, exe, url) = sys::browsers().into_iter().find(|b| b.0 == browser).ok_or("没找到这个浏览器")?;
    let url = url.trim_end_matches('/').to_string();
    arboard::Clipboard::new().and_then(|mut c| c.set_text(url.clone())).map_err(|e| format!("复制失败：{e}"))?;
    std::process::Command::new(exe).spawn().map_err(|e| format!("打不开 {name}：{e}"))?;
    Ok(url)
}

/// 浏览器里是不是已经打开了扩展管理页。
#[tauri::command]
pub fn ext_page_open() -> bool {
    sys::extensions_page_open()
}

/// 只把扩展文件夹的路径再复制一次。
#[tauri::command]
pub fn ext_copy_path() -> Result<String, String> {
    let dir = sys::extension_dir().ok_or("没找到扩展文件夹")?.display().to_string();
    arboard::Clipboard::new().and_then(|mut c| c.set_text(dir.clone())).map_err(|e| format!("复制失败：{e}"))?;
    Ok(dir)
}

/// 程序版本号（设置页底部显示）。
#[tauri::command]
pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[tauri::command]
pub fn free_space(dir: String) -> Option<u64> {
    sys::free_space(std::path::Path::new(&dir))
}
