import { defineConfig } from '@rsbuild/core';

// 现架构：axum(Rust) 同源服务前端静态资源(嵌入) + /api，主窗口加载随机高位端口。
// rsbuild 仅负责把现有静态 HTML(index/wallpaper/desktop-lyrics) 与 vendor/assets 拷到
// dist，并构建 Tauri 桥接层 bridge.ts → /static/js/bridge.js（被各 HTML 引用）。
// 不再需要 dev server / 代理 / 固定端口。
export default defineConfig({
  source: {
    entry: {
      bridge: './src/bridge/index.ts',
    },
  },
  output: {
    distPath: { root: 'dist' },
    filenameHash: false,
    injectStyles: false,
  },
  tools: {
    htmlPlugin: false,
  },
});
