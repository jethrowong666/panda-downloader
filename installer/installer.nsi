; 熊猫下载器 安装程序（NSIS 3）
; 编译：makensis /DVERSION=0.2.1 /DSTAGING=<打包目录> /DOUTFILE=<输出文件> installer.nsi
; 装在当前用户目录下，不需要管理员权限。

Unicode true
ManifestDPIAware true
RequestExecutionLevel user
SetCompressor /SOLID lzma
SetCompressorDictSize 64

!ifndef VERSION
  !define VERSION "0.2.1"
!endif
!define APPNAME "熊猫下载器"
!define APPEXE "PandaDownloader.exe"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\PandaDownloader"
!define HOSTNAME "com.panda.downloader"

Name "${APPNAME}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\PandaDownloader"
InstallDirRegKey HKCU "${UNINSTKEY}" "InstallLocation"
BrandingText "${APPNAME} ${VERSION}"
VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${APPNAME}"
VIAddVersionKey "FileDescription" "${APPNAME} 安装程序"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "GPL-3.0-or-later"

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "x64.nsh"

!define MUI_ICON "icon.ico"
!define MUI_UNICON "icon.ico"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TITLE "欢迎安装 ${APPNAME} ${VERSION}"
!define MUI_WELCOMEPAGE_TEXT "全场景下载器：多连接加速、断点续传，支持迅雷 / 快车 / 旋风链接和 B 站、YouTube 等视频网站。$\r$\n$\r$\n安装在当前用户下，不需要管理员权限。$\r$\n$\r$\n点「下一步」继续。"
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APPEXE}"
!define MUI_FINISHPAGE_RUN_TEXT "立即打开熊猫下载器"
!define MUI_FINISHPAGE_SHOWREADME "$INSTDIR\使用说明.txt"
!define MUI_FINISHPAGE_SHOWREADME_TEXT "查看使用说明（含浏览器扩展安装步骤）"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "${APPNAME} 需要 64 位 Windows 10 或更新版本。"
    Abort
  ${EndIf}
  ; WebView2 运行时（Windows 11 自带；Windows 10 一般随 Edge 装好了）
  ReadRegStr $0 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  ${If} $0 == ""
    ReadRegStr $0 HKCU "Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" "pv"
  ${EndIf}
  ${If} $0 == ""
  ${OrIf} $0 == "0.0.0.0"
    MessageBox MB_YESNO|MB_ICONEXCLAMATION "这台电脑缺少 Microsoft Edge WebView2 运行时，熊猫下载器的界面需要它。$\r$\n$\r$\n现在打开微软官网下载吗？（装好后再运行本安装程序）" IDNO +2
    ExecShell "open" "https://developer.microsoft.com/microsoft-edge/webview2/"
    Abort
  ${EndIf}
FunctionEnd

Section "主程序" SecMain
  SectionIn RO
  ; 正在运行的话先关掉，否则文件替换不了
  nsExec::Exec 'taskkill /F /IM ${APPEXE}'
  nsExec::Exec 'taskkill /F /IM panda-host.exe'
  Sleep 500

  SetOutPath "$INSTDIR"
  File "${STAGING}\${APPEXE}"
  File "${STAGING}\panda-host.exe"
  File "${STAGING}\使用说明.txt"
  SetOutPath "$INSTDIR\tools"
  File "${STAGING}\tools\yt-dlp.exe"
  File "${STAGING}\tools\ffmpeg.exe"
  File "${STAGING}\tools\deno.exe"
  ; 先清掉旧版本的扩展文件，免得留下新版已经不用的脚本
  RMDir /r "$INSTDIR\extension"
  SetOutPath "$INSTDIR\extension"
  File /r "${STAGING}\extension\*"
  SetOutPath "$INSTDIR"

  ; 给 Chrome / Edge 注册扩展通信组件（由程序自己写，保证 UTF-8 编码正确）
  ExecWait '"$INSTDIR\${APPEXE}" --register-host'

  ; 快捷方式
  CreateDirectory "$SMPROGRAMS\${APPNAME}"
  CreateShortcut "$SMPROGRAMS\${APPNAME}\${APPNAME}.lnk" "$INSTDIR\${APPEXE}"
  CreateShortcut "$SMPROGRAMS\${APPNAME}\使用说明.lnk" "$INSTDIR\使用说明.txt"
  CreateShortcut "$SMPROGRAMS\${APPNAME}\打开浏览器扩展文件夹.lnk" "$INSTDIR\extension"
  CreateShortcut "$SMPROGRAMS\${APPNAME}\卸载 ${APPNAME}.lnk" "$INSTDIR\uninstall.exe"
  CreateShortcut "$DESKTOP\${APPNAME}.lnk" "$INSTDIR\${APPEXE}"

  ; "设置 → 应用"里的卸载项
  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayName" "${APPNAME}"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTKEY}" "Publisher" "Panda"
  WriteRegStr HKCU "${UNINSTKEY}" "DisplayIcon" "$INSTDIR\${APPEXE}"
  WriteRegStr HKCU "${UNINSTKEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTKEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTKEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTKEY}" "NoRepair" 1
  WriteRegDWORD HKCU "${UNINSTKEY}" "EstimatedSize" 358000
SectionEnd

Section "Uninstall"
  nsExec::Exec 'taskkill /F /IM ${APPEXE}'
  nsExec::Exec 'taskkill /F /IM panda-host.exe'
  Sleep 500

  Delete "$INSTDIR\${APPEXE}"
  Delete "$INSTDIR\panda-host.exe"
  Delete "$INSTDIR\使用说明.txt"
  Delete "$INSTDIR\uninstall.exe"
  RMDir /r "$INSTDIR\tools"
  RMDir /r "$INSTDIR\extension"
  RMDir "$INSTDIR"

  Delete "$DESKTOP\${APPNAME}.lnk"
  RMDir /r "$SMPROGRAMS\${APPNAME}"

  ; 扩展通信组件的注册
  DeleteRegKey HKCU "Software\Google\Chrome\NativeMessagingHosts\${HOSTNAME}"
  DeleteRegKey HKCU "Software\Microsoft\Edge\NativeMessagingHosts\${HOSTNAME}"
  DeleteRegKey HKCU "Software\Chromium\NativeMessagingHosts\${HOSTNAME}"
  DeleteRegKey HKCU "Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\${HOSTNAME}"
  DeleteRegKey HKCU "${UNINSTKEY}"
  ; 设置和下载记录保留在 %APPDATA%\com.panda.downloader，重装后还在；下载的文件不动。
SectionEnd
