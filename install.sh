#!/bin/bash
# Mineradio 一键安装脚本 (macOS, Apple Silicon)
#
#   /bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/029527/Mineradio/main/install.sh)"
#
# 从 GitHub 最新 Release 拉取 .dmg, 安装到 /Applications, 并去掉隔离属性
# (应用未签名, 去隔离可绕过 Gatekeeper "无法打开" 弹窗)。
set -euo pipefail

REPO="029527/Mineradio"
APP_NAME="Mineradio"
APP_PATH="/Applications/${APP_NAME}.app"

info() { printf '\033[1;36m==>\033[0m %s\n' "$*"; }
ok()   { printf '\033[1;32m✓\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31m✗ %s\033[0m\n' "$*" >&2; exit 1; }

# 1. 平台检查 ----------------------------------------------------------------
[ "$(uname -s)" = "Darwin" ] || die "仅支持 macOS"
ARCH="$(uname -m)"
[ "$ARCH" = "arm64" ] || die "当前仅提供 Apple 芯片 (arm64) 版本，检测到: $ARCH"

# 2. 查询最新 Release 的 dmg 下载地址 ----------------------------------------
info "查询最新版本…"
API="https://api.github.com/repos/${REPO}/releases/latest"
META="$(curl -fsSL "$API")" || die "无法访问 GitHub API (${API})"
TAG="$(printf '%s' "$META" | grep -oE '"tag_name"[[:space:]]*:[[:space:]]*"[^"]+"' | head -1 | sed -E 's/.*"([^"]+)"$/\1/' || true)"
DMG_URL="$(printf '%s' "$META" | grep -oE '"browser_download_url"[[:space:]]*:[[:space:]]*"[^"]+\.dmg"' | head -1 | sed -E 's/.*"(https[^"]+)"$/\1/' || true)"
[ -n "$DMG_URL" ] || die "最新 Release 里没有找到 .dmg 文件"
ok "最新版本: ${TAG:-未知}"

# 3. 下载 dmg ----------------------------------------------------------------
TMP="$(mktemp -d)"
trap 'hdiutil detach "$TMP/mnt" -quiet >/dev/null 2>&1 || true; rm -rf "$TMP"' EXIT
DMG="$TMP/${APP_NAME}.dmg"
info "下载安装包…"
curl -fL --progress-bar "$DMG_URL" -o "$DMG" || die "下载失败"
hdiutil imageinfo "$DMG" >/dev/null 2>&1 || die "下载的文件不是有效的 dmg（可能被网络拦截返回了网页）"

# 4. 挂载 --------------------------------------------------------------------
info "挂载并安装…"
MNT="$TMP/mnt"; mkdir -p "$MNT"
hdiutil attach -nobrowse -noverify -quiet -mountpoint "$MNT" "$DMG" || die "挂载失败"
SRC_APP="$(/bin/ls -d "$MNT"/*.app 2>/dev/null | head -1 || true)"
[ -n "$SRC_APP" ] || die "dmg 里没有找到 .app"

# 5. 退出正在运行的旧版 ------------------------------------------------------
osascript -e "quit app \"${APP_NAME}\"" >/dev/null 2>&1 || true
sleep 1

# 6. 拷贝到 /Applications (必要时用 sudo) ------------------------------------
if [ -w "/Applications" ]; then
  rm -rf "$APP_PATH"
  cp -R "$SRC_APP" /Applications/
else
  info "写入 /Applications 需要管理员权限"
  sudo rm -rf "$APP_PATH"
  sudo cp -R "$SRC_APP" /Applications/
fi
hdiutil detach "$MNT" -quiet >/dev/null 2>&1 || true

# 7. 去隔离 (未签名应用) -----------------------------------------------------
xattr -dr com.apple.quarantine "$APP_PATH" 2>/dev/null || true

ok "安装完成: $APP_PATH"
info "启动 ${APP_NAME}…"
open "$APP_PATH" || true
