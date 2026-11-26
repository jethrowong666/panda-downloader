"""打包浏览器扩展的商店版。

    python scripts/pack_extension.py

生成 dist/store/panda-extension-edge-<版本>.zip 和 panda-extension-chrome-<版本>.zip。

和安装包里自带的扩展相比，商店版有两处不同：
1. 去掉 manifest 里的 "key"（商店会自己分配扩展 ID；上传带 key 的包会被拒）。
2. Chrome 版屏蔽 YouTube：Chrome 应用商店不允许扩展下载 YouTube 视频。
   - 页面脚本不注入 YouTube（exclude_matches），视频上不会出现「熊猫下载」按钮；
   - 后台脚本的 STORE_BLOCK 填上 YouTube 的域名，不嗅探、不接右键菜单。
   Edge 版不屏蔽。
"""
import json
import pathlib
import re
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
SRC = ROOT / "extension"
OUT = ROOT / "dist" / "store"

YOUTUBE = ["youtube.com", "youtu.be", "googlevideo.com", "youtube-nocookie.com"]
YOUTUBE_MATCHES = ["*://*.youtube.com/*", "*://youtu.be/*", "*://*.youtube-nocookie.com/*"]


def build(store: str) -> pathlib.Path:
    manifest = json.loads((SRC / "manifest.json").read_text(encoding="utf-8"))
    manifest.pop("key", None)
    files: dict[str, bytes] = {}
    for p in sorted(SRC.rglob("*")):
        if p.is_file():
            files[p.relative_to(SRC).as_posix()] = p.read_bytes()

    if store == "chrome":
        for cs in manifest.get("content_scripts", []):
            if any(m in ("http://*/*", "https://*/*", "<all_urls>") for m in cs.get("matches", [])):
                cs["exclude_matches"] = YOUTUBE_MATCHES
        sw = files["sw.js"].decode("utf-8")
        new, n = re.subn(r"const STORE_BLOCK = \[\];", "const STORE_BLOCK = " + json.dumps(YOUTUBE) + ";", sw)
        assert n == 1, "sw.js 里没找到 STORE_BLOCK"
        files["sw.js"] = new.encode("utf-8")

    files["manifest.json"] = (json.dumps(manifest, ensure_ascii=False, indent=2) + "\n").encode("utf-8")
    OUT.mkdir(parents=True, exist_ok=True)
    out = OUT / f"panda-extension-{store}-{manifest['version']}.zip"
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in files.items():
            z.writestr(name, data)
    return out


if __name__ == "__main__":
    for store in ("edge", "chrome"):
        p = build(store)
        with zipfile.ZipFile(p) as z:
            m = json.loads(z.read("manifest.json"))
            sw = z.read("sw.js").decode("utf-8")
        blocked = "youtube.com" in re.search(r"const STORE_BLOCK = (\[.*?\]);", sw).group(1)
        print(f"{p.name}: {p.stat().st_size // 1024} KB, 版本 {m['version']}, 带 key={'key' in m}, 屏蔽 YouTube={blocked}")
