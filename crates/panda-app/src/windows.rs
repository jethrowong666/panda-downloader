//! 四种窗口：确认窗（鼠标旁）、悬浮窗（右上角）、完成窗（右下角）、主窗口。
//!
//! 悬浮窗和完成窗每次出现都重新创建（创建时不抢焦点），消失时直接关闭。
//! 这样它们出现时不会打断用户正在打字或玩的东西。

use tauri::{AppHandle, LogicalPosition, Manager, WebviewUrl, WebviewWindowBuilder};

const FLOAT_W: f64 = 316.0;
const DONE_W: f64 = 360.0;
const CONFIRM_W: f64 = 480.0;
const MARGIN: f64 = 16.0;

/// 主显示器的可用区域（不含任务栏），逻辑像素：(x, y, 宽, 高, 缩放)。
fn work_area(app: &AppHandle) -> (f64, f64, f64, f64, f64) {
    if let Ok(Some(m)) = app.primary_monitor() {
        let s = m.scale_factor();
        let wa = m.work_area();
        return (
            wa.position.x as f64 / s,
            wa.position.y as f64 / s,
            wa.size.width as f64 / s,
            wa.size.height as f64 / s,
            s,
        );
    }
    (0.0, 0.0, 1920.0, 1040.0, 1.0)
}

fn popup<'a>(app: &'a AppHandle, label: &'a str, page: &str, w: f64, h: f64) -> WebviewWindowBuilder<'a, tauri::Wry, AppHandle> {
    WebviewWindowBuilder::new(app, label, WebviewUrl::App(page.into()))
        .title("熊猫下载器")
        .inner_size(w, h)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        // 关掉系统级拖放，网页才能收到拖进来的链接文字。
        .disable_drag_drop_handler()
}

pub fn set_float_visible(app: &AppHandle, visible: bool, saved_pos: Option<(i32, i32)>) {
    let existing = app.get_webview_window("float");
    match (visible, existing) {
        (true, None) => {
            let (x, y, w, _, s) = work_area(app);
            let (px, py) = match saved_pos {
                Some((px, py)) => (px as f64 / s, py as f64 / s),
                None => (x + w - FLOAT_W - MARGIN, y + MARGIN),
            };
            let _ = popup(app, "float", "float.html", FLOAT_W, 120.0).position(px, py).build();
        }
        (false, Some(win)) => {
            let _ = win.close();
        }
        _ => {}
    }
}

/// 悬浮窗始终在最上面：别的程序后来也设了"置顶"时会盖住我们，所以定期再抬一次。
/// 不抢焦点、不移动、不改大小；异步提交，不会卡住调用线程。
pub fn keep_float_on_top(app: &AppHandle) {
    #[cfg(windows)]
    if let Some(win) = app.get_webview_window("float") {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, HWND_TOPMOST, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE,
        };
        if let Ok(h) = win.hwnd() {
            unsafe {
                SetWindowPos(
                    h.0 as _,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS,
                );
            }
        }
    }
    #[cfg(not(windows))]
    let _ = app;
}

pub fn set_done_visible(app: &AppHandle, visible: bool) {
    let existing = app.get_webview_window("done");
    match (visible, existing) {
        (true, None) => {
            let (x, y, w, h, _) = work_area(app);
            let height = 176.0;
            let _ = popup(app, "done", "done.html", DONE_W, height)
                .position(x + w - DONE_W - MARGIN, y + h - height - MARGIN)
                .build();
        }
        (false, Some(win)) => {
            let _ = win.close();
        }
        _ => {}
    }
}

/// 完成窗内容变高时，保持底边贴着屏幕底部。
pub fn anchor_done_bottom(app: &AppHandle, height: f64) {
    if let Some(win) = app.get_webview_window("done") {
        let (x, y, w, h, _) = work_area(app);
        let _ = win.set_size(tauri::LogicalSize::new(DONE_W, height));
        let _ = win.set_position(LogicalPosition::new(x + w - DONE_W - MARGIN, y + h - height - MARGIN));
    }
}

/// 确认窗出现在鼠标附近，不用把鼠标移到屏幕中间去点。
pub fn open_confirm(app: &AppHandle, group: u64) {
    let label = format!("confirm-{group}");
    if app.get_webview_window(&label).is_some() {
        return;
    }
    let (ax, ay, aw, ah, s) = work_area(app);
    let h = 300.0;
    let (cx, cy) = crate::sys::cursor_pos().map(|(x, y)| (x as f64 / s, y as f64 / s)).unwrap_or((ax + aw / 2.0, ay + ah / 3.0));
    let x = (cx - CONFIRM_W / 2.0).clamp(ax + MARGIN, ax + aw - CONFIRM_W - MARGIN);
    let y = (cy + 24.0).clamp(ay + MARGIN, ay + ah - h - 120.0);
    let _ = popup(app, &label, &format!("confirm.html?g={group}"), CONFIRM_W, h)
        .position(x, y)
        .skip_taskbar(false)
        .focused(true)
        .build()
        .map(|w| {
            let _ = w.set_focus();
        });
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("main.html".into()))
        .title("熊猫下载器")
        .inner_size(960.0, 640.0)
        .min_inner_size(720.0, 480.0)
        .center()
        .disable_drag_drop_handler()
        .build();
}
