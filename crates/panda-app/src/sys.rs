//! 和操作系统打交道的小功能：全屏检测、鼠标位置、打开文件、杀毒扫描、"来自网络"标记、解压。

use std::path::{Path, PathBuf};

/// 用户正在全屏玩游戏、看视频、演示，或者开了"勿扰"——这时候不弹任何窗口。
#[cfg(windows)]
pub fn user_is_busy() -> bool {
    use windows_sys::Win32::UI::Shell::SHQueryUserNotificationState;
    let mut state = 0;
    // 2 = QUNS_BUSY（全屏程序），3 = QUNS_RUNNING_D3D_FULL_SCREEN（全屏游戏），4 = QUNS_PRESENTATION_MODE（演示模式）
    unsafe { SHQueryUserNotificationState(&mut state) == 0 && matches!(state, 2..=4) }
}
#[cfg(not(windows))]
pub fn user_is_busy() -> bool {
    false
}

/// 鼠标当前位置（物理像素）。
#[cfg(windows)]
pub fn cursor_pos() -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
}
#[cfg(not(windows))]
pub fn cursor_pos() -> Option<(i32, i32)> {
    None
}

/// 用系统默认程序打开文件或文件夹。
#[cfg(windows)]
pub fn shell_open(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let r = unsafe {
        ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), wide.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL)
    };
    // 返回值大于 32 表示成功
    if r as isize > 32 {
        Ok(())
    } else {
        Err(format!("无法打开 {}", path.display()))
    }
}
#[cfg(not(windows))]
pub fn shell_open(path: &Path) -> Result<(), String> {
    std::process::Command::new("xdg-open").arg(path).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// 打开所在文件夹并选中这个文件。
pub fn reveal(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if path.exists() {
            return std::process::Command::new("explorer")
                .raw_arg(format!("/select,\"{}\"", path.display()))
                .spawn()
                .map(|_| ())
                .map_err(|e| e.to_string());
        }
    }
    match path.parent() {
        Some(dir) => shell_open(dir),
        None => Err("找不到所在文件夹".into()),
    }
}

/// 给下载的文件打上"来自网络"标记（Zone.Identifier），
/// 这样 Windows SmartScreen 和 Office 受保护视图会照常检查它，和浏览器下载的文件一样安全。
/// 文件或整个文件夹的大小。
pub fn size_of(path: &Path) -> u64 {
    match std::fs::metadata(path) {
        Ok(m) if m.is_dir() => std::fs::read_dir(path)
            .map(|rd| rd.flatten().map(|e| size_of(&e.path())).sum())
            .unwrap_or(0),
        Ok(m) => m.len(),
        Err(_) => 0,
    }
}

pub fn mark_from_internet(path: &Path, url: &str) {
    #[cfg(windows)]
    {
        // 只给已存在的文件打标记（文件夹不行；路径不存在时写 ADS 会凭空建出一个空文件）
        if !path.is_file() {
            return;
        }
        let mut ads = path.as_os_str().to_owned();
        ads.push(":Zone.Identifier");
        let host_url: String = url.chars().filter(|c| !c.is_control()).collect();
        let _ = std::fs::write(PathBuf::from(ads), format!("[ZoneTransfer]\r\nZoneId=3\r\nHostUrl={host_url}\r\n"));
    }
    #[cfg(not(windows))]
    let _ = (path, url);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanResult {
    Clean,
    Threat,
    /// 没有 Defender 或扫描失败，无法判断。
    Unknown,
}

/// 用 Windows Defender 扫描单个文件。
pub fn defender_scan(path: &Path) -> ScanResult {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let exe = Path::new(r"C:\Program Files\Windows Defender\MpCmdRun.exe");
        if !exe.exists() {
            return ScanResult::Unknown;
        }
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let status = std::process::Command::new(exe)
            .args(["-Scan", "-ScanType", "3", "-DisableRemediation", "-File"])
            .arg(path)
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        match status.map(|s| s.code()) {
            Ok(Some(0)) => ScanResult::Clean,
            Ok(Some(2)) => ScanResult::Threat,
            _ => ScanResult::Unknown,
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        ScanResult::Unknown
    }
}

/// 把 zip 解压到同目录下的同名文件夹（已存在则加编号），返回解压目录。
pub fn extract_zip(path: &Path) -> Result<PathBuf, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("不是有效的 zip 文件: {e}"))?;
    let parent = path.parent().ok_or("找不到所在文件夹")?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "解压".into());
    let mut dest = parent.join(&stem);
    let mut n = 1;
    while dest.exists() {
        dest = parent.join(format!("{stem} ({n})"));
        n += 1;
    }
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        // enclosed_name 会拒绝 "../" 之类的恶意路径（zip slip）。
        let Some(rel) = entry.enclosed_name() else { continue };
        let out = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else {
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            let mut f = std::fs::File::create(&out).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
        }
    }
    Ok(dest)
}

/// 移动文件；跨磁盘时改为复制后删除。
pub fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(p) = to.parent() {
        std::fs::create_dir_all(p)?;
    }
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
    }
}

/// 目录所在磁盘的剩余空间（字节）。
#[cfg(windows)]
pub fn free_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    // 目录可能还没建，往上找到存在的那一级。
    let mut d = dir.to_path_buf();
    while !d.exists() {
        d = d.parent()?.to_path_buf();
    }
    let wide: Vec<u16> = d.as_os_str().encode_wide().chain([0]).collect();
    let mut free = 0u64;
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(free)
}
#[cfg(not(windows))]
pub fn free_space(_dir: &Path) -> Option<u64> {
    None
}
