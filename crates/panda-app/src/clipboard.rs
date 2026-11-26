//! 剪贴板监听：复制了下载链接就自动弹出确认窗，不用打开软件再粘贴。
//!
//! 只认"看起来是文件"的链接（带 zip、mp4、exe 等扩展名）、迅雷/快车/旋风专用链接、
//! 磁力链接和种子网址，复制普通网页地址不会打扰用户。

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
    match p.engine() {
        panda_link::Engine::Http => !p.wrappers.is_empty() || category::looks_downloadable(&p.resolved),
        // 磁力链接、种子网址要；单独一串 40 位十六进制（可能是 git 提交号之类）不要
        panda_link::Engine::BitTorrent => {
            let l = p.resolved.trim().to_ascii_lowercase();
            l.starts_with("magnet:") || ((l.starts_with("http://") || l.starts_with("https://")) && l.split(['?', '#']).next().is_some_and(|u| u.ends_with(".torrent")))
        }
        _ => false,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_bt_links() {
        assert!(worth_capturing("magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10&dn=Sintel"));
        assert!(worth_capturing("https://a.com/x/Ubuntu.iso.torrent"));
        assert!(worth_capturing("https://a.com/x/Ubuntu.iso.torrent?key=1"));
        assert!(worth_capturing("https://a.com/file.zip"));
        // 不打扰：普通网页、单独的哈希、本地种子路径
        assert!(!worth_capturing("https://a.com/article"));
        assert!(!worth_capturing("08ada5a7a6183aae1e09d831df6748d566095a10"));
        assert!(!worth_capturing(r"C:\种子.torrent"));
        // 从一段文字里提取出磁力链接
        let links = panda_link::extract_links("快下载：magnet:?xt=urn:btih:08ada5a7a6183aae1e09d831df6748d566095a10&dn=Sintel 谢谢");
        assert!(links.iter().any(|l| worth_capturing(l)), "{links:?}");
    }
}
