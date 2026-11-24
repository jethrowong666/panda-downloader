//! 剪贴板监听：复制了下载链接就自动弹出确认窗，不用打开软件再粘贴。
//!
//! 只认"看起来是文件"的链接（带 zip、mp4、exe 等扩展名）和迅雷/快车/旋风专用链接，
//! 复制普通网页地址不会打扰用户。

use std::sync::Arc;
use std::time::Duration;

use crate::category;
use crate::manager::Manager;

pub fn start(m: Arc<Manager>) {
    std::thread::Builder::new()
        .name("clipboard".into())
        .spawn(move || {
            let Ok(mut cb) = arboard::Clipboard::new() else { return };
            // 启动时剪贴板里已有的内容不算。
            let mut last = cb.get_text().unwrap_or_default();
            loop {
                std::thread::sleep(Duration::from_millis(600));
                let Ok(text) = cb.get_text() else { continue };
                if text == last {
                    continue;
                }
                last = text.clone();
                if !m.settings().watch_clipboard || text.len() > 200_000 {
                    continue;
                }
                let links: Vec<String> = panda_link::extract_links(&text).into_iter().filter(|l| worth_capturing(l)).collect();
                if !links.is_empty() {
                    m.capture(links);
                }
            }
        })
        .ok();
}

fn worth_capturing(link: &str) -> bool {
    let Ok(p) = panda_link::parse(link) else { return false };
    if p.engine() != panda_link::Engine::Http {
        return false;
    }
    !p.wrappers.is_empty() || category::looks_downloadable(&p.resolved)
}

/// 快捷键 Ctrl+Alt+D：下载剪贴板里的链接（任何 http 链接都算）。
pub fn capture_now(m: &Arc<Manager>) -> usize {
    let Ok(mut cb) = arboard::Clipboard::new() else { return 0 };
    let text = cb.get_text().unwrap_or_default();
    let links = panda_link::extract_links(&text);
    let n = links.len();
    if n > 0 {
        m.capture(links);
    }
    n
}
