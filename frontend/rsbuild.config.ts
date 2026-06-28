import { defineConfig } from '@rsbuild/core';

// 后端 axum 在开发期固定监听该端口（生产期由 Rust 探测空闲端口并同源服务）。
const DEV_API_PORT = Number(process.env.MINERADIO_DEV_API_PORT || 3000);

export default defineConfig({
  // 多页面：主界面 / 桌面歌词覆盖层 / 壁纸覆盖层
  source: {
    entry: {
      index: './src/index.ts',
    },
  },
  html: {
    template: './src/index.html',
  },
  server: {
    port: 1420,
    strictPort: true,
    // 开发期把 /api 转发到本地 axum 后端，保持前端同源 fetch('/api/...') 不变
    proxy: {
      '/api': `http://127.0.0.1:${DEV_API_PORT}`,
    },
  },
  output: {
    distPath: {
      root: 'dist',
    },
  },
});
