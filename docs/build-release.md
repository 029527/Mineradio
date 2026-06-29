# 发版指南（macOS）

本仓库脱离上游单独发版，安装包**手动构建、手动上传到 GitHub Release**。
用户侧的一键安装脚本 [`install.sh`](../install.sh) 始终指向「最新 Release」，因此发版只需保证新版本是 `latest` 即可，脚本无需改动。

> 适用平台：仅 macOS（Apple 芯片 / aarch64）。

## 前置条件

- 已安装 Rust 工具链、bun、Xcode Command Line Tools（详见 [README 开发者章节](../README.md#开发者)）。
- 已安装并登录 GitHub CLI：`gh auth status` 显示登录到 `029527`。
- 本地工作区干净，且代码在要发布的分支上。

## 步骤

### 1. 升版本号

两处版本号要保持一致（决定 dmg 文件名与应用内 `currentVersion`）：

| 文件 | 字段 |
| --- | --- |
| `src-tauri/tauri.conf.json` | `version`（决定 dmg 文件名 `Mineradio_<版本>_aarch64.dmg`） |
| `src-tauri/Cargo.toml` | `version`（`env!("CARGO_PKG_VERSION")`，应用内显示的当前版本） |

提交一次版本号变更，例如：

```bash
git commit -am "chore: 升级版本号到 1.2.0"
```

### 2. 构建干净 dmg

```bash
bash scripts/package-macos.sh
```

该脚本会先 `bun tauri build`，再重新封装 dmg、清掉 Tauri 残留的隐藏系统文件
（`.VolumeIcon.icns` / `.fseventsd` / `.Trashes` 等），产出窗口干净、无多余滚动条的安装包。

> ⚠️ 不要直接用 `bun tauri build` 发版——它生成的 dmg 在开启「显示隐藏文件」时会露出杂散图标，还可能撑出横向滚动条。务必走 `scripts/package-macos.sh`。

产物路径：

```
src-tauri/target/release/bundle/dmg/Mineradio_<版本>_aarch64.dmg
```

### 3. 创建 Release 并上传 dmg

```bash
VERSION=1.2.0
DMG="src-tauri/target/release/bundle/dmg/Mineradio_${VERSION}_aarch64.dmg"

gh release create "v${VERSION}" "$DMG" \
  -R 029527/Mineradio \
  --target "$(git branch --show-current)" \
  -t "Mineradio v${VERSION}" \
  -n "macOS (Apple 芯片) 安装包。

一键安装:
\`\`\`bash
/bin/bash -c \"\$(curl -fsSL https://raw.githubusercontent.com/029527/Mineradio/main/install.sh)\"
\`\`\`
或手动下载下方 dmg，拖入 Applications。"
```

发布后立即生效：

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/029527/Mineradio/main/install.sh)"
```

## 工作原理

- **`install.sh`**：调用 `https://api.github.com/repos/029527/Mineradio/releases/latest`，
  取其中 `.dmg` 资产的下载地址 → 下载并校验 → 挂载 → 拷贝 `Mineradio.app` 到 `/Applications`
  → `xattr -dr com.apple.quarantine` 去掉隔离属性（应用未签名，去隔离可绕过 Gatekeeper 拦截）→ 启动。
- **应用内更新检测**：`src-tauri/src/server/update.rs` 的 `/api/update/latest` 比较
  GitHub 最新 Release 的 `tag_name` 与本地 `CARGO_PKG_VERSION`，决定是否提示有新版本。
  因此**新版本的 tag 必须是数值更高的版本号**（如 `v1.2.0`），版本比较才会判定为「有更新」。

## 注意事项

- `install.sh` 同时存在于 **`main` 分支**（保证稳定 raw 入口 `main/install.sh`）和开发分支。
  改动安装脚本后，记得同步到 `main`。
- Release 的 `--target` 指向放代码的分支即可（仅用于打 tag，不影响 dmg 托管）。
- 一次只保留一个「最新稳定版」作为 `latest`；预发布请加 `--prerelease`，避免被一键脚本拉取。
