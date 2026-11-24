//! 文件分类：决定默认保存到哪个文件夹，以及下载完成后主按钮做什么。

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Archive,
    Video,
    Audio,
    Program,
    Document,
    Image,
    Other,
}

impl Category {
    pub fn key(self) -> &'static str {
        match self {
            Category::Archive => "archive",
            Category::Video => "video",
            Category::Audio => "audio",
            Category::Program => "program",
            Category::Document => "document",
            Category::Image => "image",
            Category::Other => "other",
        }
    }

    /// 默认子文件夹名。
    pub fn folder(self) -> &'static str {
        match self {
            Category::Archive => "压缩包",
            Category::Video => "视频",
            Category::Audio => "音乐",
            Category::Program => "程序",
            Category::Document => "文档",
            Category::Image => "图片",
            Category::Other => "其他",
        }
    }
}

/// 下载完成后的主按钮：用户最可能的下一步。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// 解压到同名文件夹（目前支持 zip）。
    Extract,
    /// 用默认播放器打开。
    Play,
    /// 先用 Windows Defender 扫描，安全再运行。
    Install,
    /// 用默认程序查看。
    View,
    Open,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Extract => "解压到当前文件夹",
            Action::Play => "播放",
            Action::Install => "扫描并运行",
            Action::View => "打开查看",
            Action::Open => "打开",
        }
    }
}

const ARCHIVE: &[&str] = &["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "zst", "iso", "cab"];
const VIDEO: &[&str] = &["mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "ts", "rmvb", "rm", "mpg", "mpeg", "3gp"];
const AUDIO: &[&str] = &["mp3", "flac", "wav", "aac", "m4a", "ogg", "opus", "wma", "ape", "dsf"];
const PROGRAM: &[&str] = &["exe", "msi", "msix", "appx", "apk", "dmg", "pkg", "deb", "rpm", "appimage", "bat"];
const DOCUMENT: &[&str] = &["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "epub", "mobi", "azw3", "csv", "md", "odt"];
const IMAGE: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "bmp", "svg", "tif", "tiff", "heic", "psd", "raw"];
/// 剪贴板里出现这些扩展名的链接时才弹窗；普通网页链接不打扰用户。
const OTHER_DOWNLOADABLE: &[&str] = &["torrent", "bin", "img", "jar", "whl", "crx", "xpi", "ttf", "otf", "woff2", "dll", "sys", "srt", "ass"];

pub fn ext_of(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match base.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= 9 => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

pub fn category_of(name: &str) -> Category {
    let e = ext_of(name);
    let e = e.as_str();
    if ARCHIVE.contains(&e) {
        Category::Archive
    } else if VIDEO.contains(&e) {
        Category::Video
    } else if AUDIO.contains(&e) {
        Category::Audio
    } else if PROGRAM.contains(&e) {
        Category::Program
    } else if DOCUMENT.contains(&e) {
        Category::Document
    } else if IMAGE.contains(&e) {
        Category::Image
    } else {
        Category::Other
    }
}

pub fn action_of(name: &str) -> Action {
    let e = ext_of(name);
    match category_of(name) {
        Category::Archive if e == "zip" => Action::Extract,
        Category::Video | Category::Audio => Action::Play,
        Category::Program if matches!(e.as_str(), "exe" | "msi" | "msix" | "appx") => Action::Install,
        Category::Image | Category::Document => Action::View,
        _ => Action::Open,
    }
}

/// 这个网址看起来是一个文件下载吗？（用于剪贴板监听）
pub fn looks_downloadable(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else { return false };
    let last = u.path_segments().and_then(|mut s| s.next_back()).unwrap_or("");
    let decoded = percent_decode(last);
    let e = ext_of(&decoded);
    !e.is_empty() && (category_of(&decoded) != Category::Other || OTHER_DOWNLOADABLE.contains(&e.as_str()))
}

/// 从网址猜文件名（真正的文件名要等服务器响应后才确定）。
pub fn name_from_url(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let last = u.path_segments()?.filter(|s| !s.is_empty()).next_back()?;
    let n = percent_decode(last);
    (!n.is_empty()).then_some(n)
}

fn percent_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_and_actions() {
        assert_eq!(category_of("a.ZIP"), Category::Archive);
        assert_eq!(action_of("a.zip"), Action::Extract);
        assert_eq!(action_of("a.rar"), Action::Open);
        assert_eq!(action_of("电影.mkv"), Action::Play);
        assert_eq!(action_of("setup.exe"), Action::Install);
        assert_eq!(action_of("x.pdf"), Action::View);
        assert_eq!(category_of("README"), Category::Other);
        assert_eq!(category_of(".bashrc"), Category::Other);
    }

    #[test]
    fn downloadable_links() {
        assert!(looks_downloadable("https://x.com/a/b/go1.22.zip?sig=1"));
        assert!(looks_downloadable("https://x.com/%E7%94%B5%E5%BD%B1.mp4"));
        assert!(looks_downloadable("https://x.com/a.torrent"));
        assert!(!looks_downloadable("https://x.com/news/article.html"));
        assert!(!looks_downloadable("https://x.com/"));
        assert!(!looks_downloadable("https://x.com/page"));
    }
}
