//! 四种窗口：确认窗（鼠标旁）、悬浮窗（右上角）、完成窗（右下角）、主窗口。
//!
//! 悬浮窗和完成窗每次出现都重新创建（创建时不抢焦点），消失时直接关闭。
//! 这样它们出现时不会打断用户正在打字或玩的东西。

use tauri::{AppHandle, LogicalPosition, Manager, WebviewUrl, WebviewWindowBuilder};

/// 悬浮窗默认是小窗口（只显示总速度），点开后变宽。
const FLOAT_PILL_W: f64 = 172.0;
const FLOAT_PILL_H: f64 = 64.0;
const DONE_W: f64 = 372.0;
const CONFIRM_W: f64 = 488.0;
const MARGIN: f64 = 16.0;

/// 外观设置：0 跟随系统，1 浅色，2 深色。新开的窗口按它来，已开的窗口用 `apply_theme` 切换。
static THEME: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn theme() -> Option<tauri::Theme> {
    match THEME.load(std::sync::atomic::Ordering::Relaxed) {
        1 => Some(tauri::Theme::Light),
        2 => Some(tauri::Theme::Dark),
        _ => None,
    }
}

/// 设置外观（"system" / "light" / "dark"），并让已经打开的窗口马上跟着变。
pub fn apply_theme(app: &AppHandle, name: &str) {
    let v = match name {
        "light" => 1,
        "dark" => 2,
        _ => 0,
    };
    THEME.store(v, std::sync::atomic::Ordering::Relaxed);
    for (_, w) in app.webview_windows() {
        let _ = w.set_theme(theme());
    }
}

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

/// 某一点所在显示器的可用区域（逻辑像素）：(x, y, 宽, 高, 缩放)。点不在任何显示器上就用主显示器。
fn work_area_at(app: &AppHandle, phys_x: i32, phys_y: i32) -> (f64, f64, f64, f64, f64) {
    if let Ok(list) = app.available_monitors() {
        for m in list {
            let (p, sz) = (m.position(), m.size());
            if phys_x >= p.x && phys_x < p.x + sz.width as i32 && phys_y >= p.y && phys_y < p.y + sz.height as i32 {
                let s = m.scale_factor();
                let wa = m.work_area();
                return (wa.position.x as f64 / s, wa.position.y as f64 / s, wa.size.width as f64 / s, wa.size.height as f64 / s, s);
            }
        }
    }
    work_area(app)
}

/// 把悬浮窗放回屏幕里（窗口变宽、换了分辨率、拔掉显示器后都可能跑到外面去）。
pub fn keep_float_inside(app: &AppHandle) {
    let Some(win) = app.get_webview_window("float") else { return };
    let (Ok(pos), Ok(size)) = (win.outer_position(), win.outer_size()) else { return };
    let (ax, ay, aw, ah, s) = work_area_at(app, pos.x + size.width as i32 / 2, pos.y + size.height as i32 / 2);
    let (w, h) = (size.width as f64 / s, size.height as f64 / s);
    let (x, y) = (pos.x as f64 / s, pos.y as f64 / s);
    let nx = x.clamp(ax, (ax + aw - w).max(ax));
    let ny = y.clamp(ay, (ay + ah - h).max(ay));
    if (nx - x).abs() > 0.5 || (ny - y).abs() > 0.5 {
        let _ = win.set_position(LogicalPosition::new(nx, ny));
    }
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
        .theme(theme())
        // 关掉系统级拖放，网页才能收到拖进来的链接文字。
        .disable_drag_drop_handler()
}

/// `anchor`：上次记住的右边缘和上边缘（物理像素）。没有就放在主显示器右上角。
pub fn set_float_visible(app: &AppHandle, visible: bool, anchor: Option<(i32, i32)>) {
    let existing = app.get_webview_window("float");
    match (visible, existing) {
        (true, None) => {
            let (px, py) = match anchor {
                Some((right, top)) => {
                    // 按右边缘放，再限制在那块屏幕里面
                    let (ax, ay, aw, ah, s) = work_area_at(app, right - 1, top);
                    let x = (right as f64 / s - FLOAT_PILL_W).clamp(ax, ax + aw - FLOAT_PILL_W);
                    let y = (top as f64 / s).clamp(ay, ay + ah - FLOAT_PILL_H);
                    (x, y)
                }
                None => {
                    let (x, y, w, _, _) = work_area(app);
                    (x + w - FLOAT_PILL_W - MARGIN, y + MARGIN)
                }
            };
            let _ = popup(app, "float", "float.html", FLOAT_PILL_W, FLOAT_PILL_H).position(px, py).build();
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

/// 系统支不支持 Mica 毛玻璃（Windows 11）。
fn has_mica() -> bool {
    #[cfg(windows)]
    {
        windows_version::OsVersion::current().build >= 22000
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    // Win11（22000 以上）有系统毛玻璃 Mica：窗口透明，让它透出桌面的颜色。
    // Win10 没有，窗口保持不透明，页面自己画底色（不然会直接看到后面的桌面）。
    let mica = has_mica();
    let page = if mica { "main.html?mica=1" } else { "main.html" };
    let mut b = WebviewWindowBuilder::new(app, "main", WebviewUrl::App(page.into()))
        .title("熊猫下载器")
        .inner_size(1000.0, 660.0)
        .min_inner_size(780.0, 500.0)
        .center()
        .theme(theme())
        .disable_drag_drop_handler();
    if mica {
        b = b.transparent(true).effects(tauri::window::EffectsBuilder::new().effect(tauri::window::Effect::Mica).build());
    }
    let _ = b.build();
}
