# Mineradio（macOS · Tauri 重写版）

![Mineradio 暗场启动页](./docs/assets/readme/cinema-beat-smoke.png)

Mineradio 是一款沉浸式桌面音乐播放器，把天气电台、搜索播放、歌词舞台、粒子视觉和 3D 歌单架组合成一个更接近现场感的私人音乐空间。

> **这是一个 fork。** 本仓库 [`029527/Mineradio`](https://github.com/029527/Mineradio) fork 自上游
> [`XxHuberrr/Mineradio`](https://github.com/XxHuberrr/Mineradio)（原版为 **Windows + Electron**）。
> 本分支把整套架构从 Electron 重写到了 **Rust + Tauri 2**，并以 **macOS（Apple 芯片）** 为首要目标平台，
> 脱离上游单独发版。原作者与原项目的功能设计版权归上游所有，详见文末「上游与致谢」。

---

## 安装（macOS）

一键安装（**自动识别 Apple 芯片 / Intel**，拉取对应架构最新版、装入 `/Applications`、去掉隔离属性后启动）：

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/029527/Mineradio/main/install.sh)"
```

或手动从 [Releases](https://github.com/029527/Mineradio/releases) 下载对应架构的包，拖入「应用程序」：

| 机型 | 文件 |
| --- | --- |
| Apple 芯片 | `Mineradio_<版本>_aarch64.dmg` |
| Intel | `Mineradio_<版本>_x64.dmg` |

应用未签名，若提示「无法打开」，右键图标 → 打开，或执行：

```bash
xattr -dr com.apple.quarantine /Applications/Mineradio.app
```

---

## 从 Electron 到 Tauri：架构是什么样的

上游原版由三块构成：Electron 主进程（`desktop/main.js`）+ 本地 Node HTTP 服务（`server.js`，约 50 个 `/api` 路由）+ 浏览器侧前端（`public/index.html`，three.js / gsap）。本 fork 在保留前端体验的前提下，把**主进程和后端整体用 Rust 重写**，外壳换成 Tauri 2。

```
┌─────────────────────────────────────────────────────────────┐
│  Tauri 2 外壳 (Rust)                                          │
│  · 窗口/生命周期/红绿灯标题栏 · 全局热键 · 文件对话框        │
│  · webview 登录 · 24 个 invoke 命令                          │
│                                                              │
│   ┌────────────────────────┐      WKWebView                  │
│   │  内嵌 axum HTTP 服务    │◀────  主窗口加载                │
│   │  (随机高位端口)         │      http://127.0.0.1:<port>/   │
│   │                        │                                 │
│   │  /api/*  网易云·QQ·    │      前端 (vanilla HTML/JS)      │
│   │   天气·更新·音频代理   │      three.js · gsap            │
│   │  /        静态资源      │──▶   bun + rsbuild 构建         │
│   │  (rust-embed 嵌入)      │      bridge.ts 重建             │
│   └────────────────────────┘      window.desktopWindow       │
└─────────────────────────────────────────────────────────────┘
```

**关键设计：**

- **后端 = Rust 内嵌 axum 服务，而非纯 invoke。** 前端沿用相对路径 `fetch('/api/...')`，
  且音频要 HTTP Range（拖动进度）、封面要注入 `Referer`（绕防盗链），所以后端必须是真正的 HTTP 服务。
  Rust 在启动时绑定一个随机高位端口，主窗口直接加载 `http://127.0.0.1:<port>/`。
- **前端静态资源用 `rust-embed` 处理。** debug 期从磁盘读 `frontend/dist`（改完前端重建即可生效），
  release 期把整个前端编入二进制，打包后无需外部文件。
- **前端工具链 = bun + rsbuild。** 三个祖传页面（`index` / `wallpaper` / `desktop-lyrics`）作为静态资源原样保留，
  rsbuild 只编译一个 `bridge.ts`，在 `window.__TAURI__` 之上重建 `window.desktopWindow` 桥接，
  前端业务逻辑几乎零改动。
- **网易云加密信封用 Rust 复刻**（weapi / eapi / linuxapi，AES + RSA + MD5），与上游逐端点对拍一致。
- **播客 DJ 节拍图**用 `symphonia` 解码 MP3 + 自实现 DSP 移植，输出与原 Node 版逐项对齐。
- **平台门控：** 以 macOS 为主；Windows 专有的贴桌面壁纸、桌面歌词鼠标轮询用 `#[cfg(windows)]` 隔离，不影响 macOS 构建。

---

## 核心特性

- Open-Meteo 天气电台，按位置、城市与天气 mood 生成播放队列
- 首页聚合天气电台、每日推荐、私人电台、继续听、听歌画像与我的歌单
- 未播放时保持干净星河背景，播放后切换到歌词舞台 + 粒子舞台视觉
- 基于节奏的电影镜头视觉系统，面向长播客 / DJ 曲目的专属视觉模式
- 歌词舞台、自定义歌词、歌词位置与视觉控制
- 自定义专辑封面上传与裁剪
- 右键唤起 3D 歌单架，支持歌单队列浏览
- 网易云音乐（扫码登录、搜索、歌单、播客）与 QQ 音乐（搜索、登录态、音源补充）接入
- 基于 GitHub Releases 的更新检测

---

## 开发者

欢迎贡献。下面是把项目跑起来和构建的方式。

### 前置环境

| 工具 | 说明 |
| --- | --- |
| **Rust** | 1.77+（Tauri 2 要求），`rustup` 安装即可 |
| **bun** | 1.3+，用作前端包管理与运行时 |
| **Xcode Command Line Tools** | macOS 构建必需（`xcode-select --install`） |

> Node 版本锁定见 `.node-version`（22），但实际用 bun 作为运行时。

### 安装依赖

```bash
# 前端依赖
cd frontend && bun install && cd ..

# 根目录（含 Tauri CLI）
bun install
```

### 本地运行

```bash
bun tauri dev
```

`bun tauri dev` 会先构建前端（`cd frontend && bun run build`），再编译并启动 Rust 应用；
Rust 后端绑定随机端口、服务 `/api/*` 与前端静态资源，主窗口加载该端口。

> **前端热更新说明：** 本项目刻意未启用 HMR（避免破坏桥接注入）。改动前端后，
> 重新执行 `cd frontend && bun run build`，再在窗口里按 `⌘R` 刷新即可（或重启 `bun tauri dev`）。
> 改动 Rust 代码则需重启 `bun tauri dev`。

### 构建

```bash
# 生成 .app + .dmg（开发自测用）
bun tauri build

# 生成「干净」dmg（发版用，会清掉隐藏系统文件）
bash scripts/package-macos.sh
```

### 目录结构

```
src-tauri/                 # Rust + Tauri 2 工程
  src/
    lib.rs                 # 入口：启后端 + 建主窗口
    commands.rs            # 24 个 invoke 命令（对应原 IPC）
    login.rs               # 网易云 / QQ webview 登录 + cookie 抓取
    server/                # 内嵌 axum 后端（替换 server.js）
      mod.rs               #   路由 + 随机端口 + 静态资源
      netease/             #   加密信封 crypto.rs + 端点
      qq.rs proxy.rs weather.rs update.rs dj_analyzer.rs
frontend/                  # bun + rsbuild
  public/                  #   index / wallpaper / desktop-lyrics 三页 + vendor/assets
  src/bridge/              #   window.desktopWindow 桥接
scripts/package-macos.sh   # 干净 dmg 打包
install.sh                 # 用户一键安装脚本
docs/build-release.md      # 发版指南
```

发版流程见 [`docs/build-release.md`](./docs/build-release.md)。

---

## 上游与致谢

- 上游原项目：[`XxHuberrr/Mineradio`](https://github.com/XxHuberrr/Mineradio)（Windows / Electron 原版），产品设计与功能版权归原作者。
- 本仓库为其 fork，主要工作是架构重写（Electron → Rust + Tauri 2）与 macOS 适配。
- 许可证：[GPL-3.0](./LICENSE)，沿用上游。

> 本项目通过逆向实现的方式对接网易云、QQ 音乐等第三方服务接口，仅供学习与个人使用，请勿用于商业用途；
> 由此产生的任何责任由使用者自行承担。
