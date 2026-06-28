import { defineConfig } from '@rsbuild/core';

// 后端 axum 开发期固定端口（生产期由 Rust 探测空闲端口并同源服务）。
const DEV_API_PORT = Number(process.env.MINERADIO_DEV_API_PORT || 3000);

// 迁移策略：现有三个 HTML（index/wallpaper/desktop-lyrics，含大量祖传内联 JS）
// 作为静态资源原样保留在 public/，不经 HTML 转换，零破坏风险。rsbuild 仅负责：
//   1. 构建 Tauri 桥接层 bridge.ts → /static/js/bridge.js（被各 HTML 引用）
//   2. 开发期 dev server：把 public/ 服务到根，并代理 /api 到 axum
export default defineConfig({
  source: {
    entry: {
      bridge: './src/bridge/index.ts',
    },
  },
  output: {
    distPath: { root: 'dist' },
    // 固定文件名，便于静态 HTML 以 /static/js/bridge.js 稳定引用。
    filenameHash: false,
    // 不向 HTML 自动注入（HTML 自管引用）。
    injectStyles: false,
  },
  tools: {
    // 现有 HTML 已是完整页面，禁用 rsbuild 的 HTML 生成。
    htmlPlugin: false,
  },
  server: {
    port: 1420,
    strictPort: true,
    proxy: {
      '/api': `http://127.0.0.1:${DEV_API_PORT}`,
    },
  },
  // htmlPlugin 关闭后 HMR 客户端基建不会注入，bundle 内残留的 HMR 运行时会在
  // webview 里报错中断、导致 bridge 入口不执行。前端是静态 HTML，无需热更新，
  // 直接关掉 HMR/liveReload，让 dev 也产出干净自执行的 bridge。
  dev: {
    hmr: false,
    liveReload: false,
  },
});
