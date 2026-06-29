#!/bin/bash
# 打包 macOS（双架构）并清理 dmg 内的隐藏系统文件。
#
# 在 Apple 芯片上会交叉编译出 Intel(x86_64) 包——这是 Rust 跨架构编译，
# 不需要开 Rosetta（Rosetta 用于运行 x86 程序，构建不需要）。
#
# 背景: Tauri 的 dmg 会在卷里留下 .VolumeIcon.icns / .fseventsd / .Trashes 等隐藏项，
#       开启"显示隐藏文件"时会露出杂散图标、还可能撑出多余滚动条，这里重新封装成干净 dmg。
#
# 用法:
#   bash scripts/package-macos.sh          # 打 aarch64 + x64 两个包(默认)
#   bash scripts/package-macos.sh aarch64  # 只打 Apple 芯片
#   bash scripts/package-macos.sh x64      # 只打 Intel
set -e
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

case "${1:-all}" in
  aarch64) TARGETS=("aarch64-apple-darwin") ;;
  x64)     TARGETS=("x86_64-apple-darwin") ;;
  all)     TARGETS=("aarch64-apple-darwin" "x86_64-apple-darwin") ;;
  *) echo "用法: $0 [aarch64|x64|all]"; exit 1 ;;
esac

clean_dmg() {
  local DMG="$1"
  echo "==> 清理 dmg: $DMG"
  local TMP; TMP="$(mktemp -u /tmp/mineradio-rw-XXXX).dmg"
  hdiutil detach "/Volumes/Mineradio" -force >/dev/null 2>&1 || true
  hdiutil convert "$DMG" -format UDRW -o "$TMP" >/dev/null
  local MP; MP="$(hdiutil attach -nobrowse "$TMP" 2>/dev/null | grep -oE '/Volumes/Mineradio' | head -1)"
  [ -z "$MP" ] && { echo "挂载失败"; rm -f "$TMP"; return 1; }
  rm -rf "$MP/.VolumeIcon.icns" "$MP/.fseventsd" "$MP/.Trashes" "$MP/.background" 2>/dev/null || true
  xattr -dr com.apple.FinderInfo "$MP" 2>/dev/null || true
  sync
  hdiutil detach "$MP" -force >/dev/null 2>&1
  rm -f "$DMG"
  hdiutil convert "$TMP" -format UDZO -o "$DMG" >/dev/null
  rm -f "$TMP"
  echo "==> 干净 dmg 完成: $DMG"
}

for T in "${TARGETS[@]}"; do
  echo "==> 确保 rust target: $T"
  rustup target add "$T" >/dev/null 2>&1 || true
  echo "==> tauri build --target $T"
  bun tauri build --target "$T"
  DMG="$(ls -t "$ROOT"/src-tauri/target/"$T"/release/bundle/dmg/*.dmg 2>/dev/null | head -1)"
  [ -z "$DMG" ] && { echo "未找到 $T 的 dmg"; exit 1; }
  clean_dmg "$DMG"
done

echo "==> 全部完成"
ls -lh "$ROOT"/src-tauri/target/*/release/bundle/dmg/*.dmg 2>/dev/null || true
