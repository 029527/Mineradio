#!/bin/bash
# 打包 macOS 并清理 dmg 内的隐藏系统文件。
# 背景: Tauri 的 dmg 会在卷里留下 .VolumeIcon.icns / .fseventsd / .Trashes 等隐藏项,
#       在开启"显示隐藏文件"或某些 Finder 配置下会露出来, 还可能撑出多余滚动条。
#       这里在 `tauri build` 之后重新封装一个干净的 dmg(保留 .DS_Store 维持图标布局)。
set -e
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "==> tauri build"
bun tauri build

DMG="$(ls -t "$ROOT"/src-tauri/target/release/bundle/dmg/*.dmg 2>/dev/null | head -1)"
[ -z "$DMG" ] && { echo "未找到 dmg"; exit 1; }
echo "==> 清理 dmg: $DMG"

VOL="/Volumes/Mineradio"
TMP="$(mktemp -u /tmp/mineradio-rw-XXXX).dmg"
hdiutil detach "$VOL" -force >/dev/null 2>&1 || true
hdiutil convert "$DMG" -format UDRW -o "$TMP" >/dev/null
MP="$(hdiutil attach -nobrowse "$TMP" 2>/dev/null | grep -oE '/Volumes/Mineradio' | head -1)"
[ -z "$MP" ] && { echo "挂载失败"; rm -f "$TMP"; exit 1; }
# 删除隐藏系统项(保留 .DS_Store 维持布局), 清除卷的自定义图标标记
rm -rf "$MP/.VolumeIcon.icns" "$MP/.fseventsd" "$MP/.Trashes" "$MP/.background" 2>/dev/null || true
xattr -dr com.apple.FinderInfo "$MP" 2>/dev/null || true
sync
hdiutil detach "$MP" -force >/dev/null 2>&1
rm -f "$DMG"
hdiutil convert "$TMP" -format UDZO -o "$DMG" >/dev/null
rm -f "$TMP"
echo "==> 干净 dmg 完成: $DMG"
