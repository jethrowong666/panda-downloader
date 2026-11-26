# 熊猫下载器浏览器扩展 隐私政策

最后更新：2026 年 11 月 26 日

## 一句话

这个扩展只和你自己电脑上安装的熊猫下载器通信。它不连接任何服务器，不收集、不上传、不出售任何数据。

## 扩展会接触哪些信息，用来做什么

| 信息 | 什么时候用到 | 去了哪里 |
|---|---|---|
| 你要下载的文件或视频的地址 | 你点了下载链接、点了「熊猫下载」按钮或右键菜单时 | 只发给你电脑上的熊猫下载器 |
| 这个下载地址和它来源页面的 Cookie（登录状态） | 同上，让需要登录才能下载的文件也能下 | 只发给你电脑上的熊猫下载器；下载器用完不保存到断点记录里 |
| 来源页面的网址、标题，浏览器标识（User-Agent） | 同上，用作下载请求的来源页和默认文件名 | 只发给你电脑上的熊猫下载器 |
| 当前页面加载的视频、音频地址 | 你打开网页时，扩展从响应头里识别出媒体文件，显示在扩展弹窗里 | 只保存在浏览器内存里，关掉标签页就清除 |
| 扩展的开关设置 | 你在扩展弹窗里改设置时 | 只保存在浏览器本地（`chrome.storage.local`） |

扩展和熊猫下载器之间用浏览器的 Native Messaging 通信，数据不离开你的电脑。

## 扩展不做的事

- 不收集浏览历史，不做统计或分析
- 不向开发者或任何第三方的服务器发送数据
- 不加载远程代码，不注入广告
- 不出售、不转让任何数据

## 熊猫下载器桌面程序

桌面程序是开源软件（GPL-3.0）。它只连接你要下载的那个地址（以及 BT 下载需要的 Tracker 和其他下载者），没有账号系统，不上传使用数据。

## 联系方式

有问题请在 GitHub 仓库提 Issue。

---

# Panda Downloader Browser Extension — Privacy Policy

Last updated: November 26, 2026

This extension only communicates with the Panda Downloader desktop app installed on your own computer, through the browser's Native Messaging channel. It does not connect to any remote server and does not collect, upload or sell any data.

When you click a download link, the "熊猫下载" button on a video, or a context-menu item, the extension passes the following to the local desktop app so it can perform the download: the file or video URL, the cookies for that URL and its referring page (so that downloads requiring sign-in work), the referring page URL and title, and the browser's User-Agent. Media URLs detected on the current page are kept in memory only and are discarded when the tab is closed. Your extension settings are stored locally in `chrome.storage.local`.

The extension does not collect browsing history, does not use analytics, does not load remote code, and does not share data with the developer or any third party.

Questions: please open an issue in the GitHub repository.
