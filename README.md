# 熊猫下载器 Panda Downloader

全场景下载器：一个工具搞定 HTTP、BT/磁力、电驴 ed2k、迅雷/快车/旋风链接、m3u8 流媒体……
开源（GPL-3.0-or-later），无广告。Panda 系列产品。

> 当前版本 v0.3.3（开发中）：HTTP 多线程下载 + 断点续传、桌面程序、浏览器扩展、视频网站下载、BT 种子 / 磁力链接。ed2k、网盘等见路线图。
>
> **下载安装包**：见 [Releases](../../releases)。安装包已自带 yt-dlp、ffmpeg、Deno，不需要另外安装任何东西。

## 桌面程序

```powershell
cargo build --release -p panda-app
.\target\release\PandaDownloader.exe
```

- **复制下载链接就会弹出确认窗**，弹出的同时已经在后台下载，按回车确认即可
- 右上角悬浮窗实时显示进度和速度；取消任务 5 秒内可以撤销
- 下载完成后右下角弹窗，主按钮按文件类型变：解压、播放、扫描并运行、打开查看
- 全屏玩游戏、看视频时不弹任何窗口
- `Ctrl` + `Alt` + `D`：下载剪贴板里的链接
- 关闭主窗口会缩到托盘，下载继续；重启后自动接着下
- **视频网站**：粘贴 B 站、YouTube 等视频页网址，选清晰度后下载，音视频自动合并成一个文件；B 站会自动挑最快的 CDN 节点；
  国内网站自动直连（开着代理也不会被当成境外访问）；YouTube 先不带登录解析，必要时再用浏览器的登录状态
- **抖音、快手、西瓜视频、小红书、微博、知乎、好看视频、TikTok、Instagram、Threads、Facebook、X、Reddit、Vimeo**：在浏览器里打开视频，点视频右上角的「熊猫下载」，
  扩展直接从网页已经加载的数据里拿视频地址（这些网站的接口要签名或登录，后台解析不了）；拿不到时自动交给解析器
- 解析器直接支持（粘贴网址即可）：B 站、YouTube、Dailymotion、Pinterest、TED、AcFun、Streamable、Twitch、Bluesky、LinkedIn、Tumblr、VK、Rumble、Kick、
  Imgur、9GAG、Niconico、SoundCloud、Bandcamp、Mixcloud、虎牙、斗鱼等上千个网站
- **BT 种子 / 磁力链接**（内核 [librqbit](https://github.com/ikatson/rqbit)）：复制磁力链接、双击 `.torrent` 文件、点网页上的磁力链接，
  或者在浏览器里点种子下载，都会弹出确认窗；多文件种子自动放进以种子命名的文件夹，默认存到"BT下载"；
  暂停、继续、取消、重启后接着下都和普通任务一样。不支持 PT（私有种子会提示不支持，不下载）。
  暂时下完就停止上传（不长期做种），做种管理以后再做
- **BT 提速**（参考 qBittorrent 增强版）：公开种子自动追加公共 Tracker（内置一份，每天从 trackerslist 更新）；每个种子最多连 250 个用户；
  固定监听端口 + UPnP 让别人能连进来。
- **网络种子（HTTP 加速）**：种子带 HTTP 下载地址时（url-list / 磁力链接的 ws=，archive.org 等），BT 持续慢就自动用多线程 HTTP 引擎同时下，
  HTTP 明显更快就暂停 BT；下完交给 BT 内核逐块校验哈希，坏块自动用 BT 补。Big Buck Bunny 实测 0.19MB/s → 1.9MB/s。安装时请求一次管理员权限在防火墙放行，下载时不会弹窗；没放行时自动改用不监听、不开 DHT 的安静模式
- 不支持有 DRM 版权加密的视频（Netflix、各平台付费会员内容等）
- 从源码运行视频功能需要把 `yt-dlp.exe`、`ffmpeg.exe`、`deno.exe` 放进项目的 `tools\` 文件夹

## 已实现

**链接识别（`panda-link`）**
- 迅雷 `thunder://`、快车 `flashget://`、旋风 `qqdl://` 自动解包，支持多层嵌套和 GBK 编码的老链接
- 识别 HTTP/HTTPS、FTP/SFTP、磁力（BT v1/v2、ed2k、SHA-1 多哈希）、ed2k 文件/服务器链接、
  `.torrent`、Metalink、m3u8、DASH、RTMP/RTSP、IPFS、裸 info hash、本地种子路径
- 批量模式：`img[001-200].jpg`、`[a-z]`、`[0-100:5]`
- 从任意文字（剪贴板、网页）中提取所有链接，ed2k 文件名带空格也能正确提取

**HTTP 引擎（`panda-http`）**
- 多连接分段 + 动态劈段：先下完的连接会接手最慢连接的后半段，避免"卡在 99%"
- 断线自动重试（指数退避），404 等致命错误立即停止

**断点续传（任何情况都不白下）**
- 进度存在 `.part.json`，每秒保存一次；**先刷盘再写记录**，程序崩溃、被强杀、断电，记录都不会比数据"超前"
- 分段下载：每段单独记录，续传时只补没下完的部分，连接数可以和上次不同
- 大小未知的文件（服务器支持 Range 但不告诉大小）：顺序续传
- 服务器不支持 Range：断线、暂停都保留数据，续传时先尝试接着下，服务器不认才从头下
- 上次单连接下到一半、这次服务器支持分段了：已下的部分保留，剩下的改为多连接
- 用 ETag / Last-Modified / 文件大小判断服务器文件是否变化，变了自动作废旧数据，避免拼出坏文件
- 换一个新复制的链接（比如签名参数变了）也能接上同一个文件
- 不同网站的同名文件不会误删对方的断点
- `pandadl resume <目录>`：不用记原链接，一条命令继续目录里所有未完成的下载
- CDN 临时链接过期（401/403/410）时，回原始地址自动刷新
- 服务器不支持分段时自动退回单连接，探测请求直接复用，不浪费一次请求
- 可传入 Cookie / Referer / User-Agent / 自定义请求头；支持 HTTP/SOCKS5 代理
- 令牌桶限速，运行中可调；多任务共用即全局限速
- 预分配磁盘空间；文件名支持 RFC 5987 和 GBK，自动清理非法字符、防路径穿越、重名自动编号

## 浏览器扩展（Chrome / Edge）

开发版加载方法：

1. 先运行一次 `PandaDownloader.exe`（它会自动向浏览器注册通信组件 `panda-host.exe`）
2. 打开 `edge://extensions` 或 `chrome://extensions`，打开"开发人员模式"
3. 点"加载解压缩的扩展"，选择项目里的 `extension` 文件夹

功能：接管浏览器下载（自动带上 Cookie 和来源页；按住 Alt 点击可临时用浏览器下载）、视频嗅探（鼠标移到视频上出现"熊猫下载"按钮，
图标显示发现的视频数）、右键菜单（下载链接 / 下载这个视频 / 下载本页视频 / 下载本页所有文件）。下载器没运行时会自动在后台启动。

## 命令行用法

```powershell
cargo build --release
.\target\release\pandadl.exe get "https://example.com/file.zip" -o D:\下载 -c 16
.\target\release\pandadl.exe get "thunder://QUFodHRw...WlpaWg==" --limit 2M
.\target\release\pandadl.exe get "https://example.com/img[001-050].jpg" -o D:\图片
.\target\release\pandadl.exe parse "ed2k://|file|a.mkv|100|0123456789abcdef0123456789abcdef|/"
.\target\release\pandadl.exe extract 网页文字.txt
```

按 Ctrl+C 暂停；再次运行同样的命令，或者运行下面的命令，从断点继续：

```powershell
.\target\release\pandadl.exe resume D:\下载          # 继续所有未完成的下载
.\target\release\pandadl.exe resume D:\下载 --list   # 只看看有哪些
```

## 项目结构

```
crates/
  panda-link/   链接识别与解包（无网络依赖）
  panda-http/   HTTP/HTTPS 下载引擎
  pandadl/      命令行版
```

## 测试

```powershell
cargo test --workspace
```

集成测试会在本机起一个可控的测试服务器，覆盖分段、续传、断线重连、动态劈段、链接过期、限速、空文件、404 等场景。
`crates/pandadl/tests/crash.rs` 会在下载中途强杀 `pandadl.exe`，再用 `pandadl resume` 续传并校验文件。

## 路线图

| 版本 | 内容 |
|---|---|
| v0.1 | 链接识别、HTTP 动态分段引擎、命令行、断点续传 ✓ |
| v0.2 | Tauri 桌面程序、浏览器扩展（接管下载、视频嗅探）、视频网站下载（yt-dlp + ffmpeg，抖音/快手/西瓜走扩展） ✓ |
| v0.3 | BT / 磁力（librqbit）全速下载、magnet: 和 .torrent 关联 ✓；浏览器扩展上架商店、链接续期 |
| v0.4 | 做种管理（分享率、做种时间）、选择下载种子里的部分文件 |
| v0.5 | 自研 HLS / DASH 下载 |
| v0.6 | ed2k 自研 |
| v0.7 | 多源并行（HTTP + BT + ed2k 同一文件一起下）、Metalink、FTP/SFTP、IPFS |
| v0.8 | aria2 兼容 RPC、Web 界面、Docker |
| v0.9 | WASM 插件、网盘官方接口、RSS |
| v1.0 | Kad 网络、稳定性与安全审计 |
