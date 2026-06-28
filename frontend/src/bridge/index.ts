// Tauri 桥接层：在 Tauri webview 内重建 Electron preload 暴露的
// window.desktopWindow / window.desktopOverlay，底层走 window.__TAURI__
// （tauri.conf app.withGlobalTauri = true 注入）。
//
// 命令名与事件名沿用原 ipcMain 通道字符串，Rust commands(任务 C) 一一对应。
// 非 Tauri 环境（普通浏览器）不注入，前端自动降级为 Web 模式。

type AnyFn = (...args: any[]) => any;

interface TauriGlobal {
  core: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<any> };
  event: {
    listen: (event: string, handler: (e: { payload: any }) => void) => Promise<() => void>;
  };
}

const tauri = (window as any).__TAURI__ as TauriGlobal | undefined;

if (tauri) {
  const invoke = (cmd: string, args?: Record<string, unknown>) => tauri.core.invoke(cmd, args);

  // 把异步 listen 适配成 preload 的「同步返回取消函数」语义。
  const on = (event: string, cb: AnyFn): (() => void) => {
    if (typeof cb !== 'function') return () => {};
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    tauri.event
      .listen(event, (e) => cb(e.payload || {}))
      .then((un) => {
        if (cancelled) un();
        else unlisten = un;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  };

  // ---- window.desktopWindow（主界面）----
  (window as any).desktopWindow = {
    isDesktop: true,
    minimize: () => invoke('desktop-window-minimize'),
    toggleMaximize: () => invoke('desktop-window-toggle-maximize'),
    toggleFullscreen: () => invoke('desktop-window-toggle-fullscreen'),
    exitFullscreenWindowed: () => invoke('desktop-window-exit-fullscreen-windowed'),
    getState: () => invoke('desktop-window-get-state'),
    close: () => invoke('desktop-window-close'),
    openNeteaseMusicLogin: () => invoke('netease-music-open-login'),
    clearNeteaseMusicLogin: () => invoke('netease-music-clear-login'),
    openQQMusicLogin: () => invoke('qq-music-open-login'),
    clearQQMusicLogin: () => invoke('qq-music-clear-login'),
    openUpdateInstaller: (filePath: string) =>
      invoke('mineradio-open-update-installer', { filePath }),
    restartApp: () => invoke('mineradio-restart-app'),
    configureGlobalHotkeys: (bindings: unknown) =>
      invoke('mineradio-hotkeys-configure-global', { bindings: bindings || [] }),
    exportJsonFile: (payload: unknown) =>
      invoke('mineradio-export-json-file', { payload: payload || {} }),
    importJsonFile: () => invoke('mineradio-import-json-file'),
    onGlobalHotkey: (cb: AnyFn) => on('mineradio-global-hotkey', cb),
    setDesktopLyricsEnabled: (enabled: boolean, payload: unknown) =>
      invoke('mineradio-desktop-lyrics-set-enabled', { enabled: !!enabled, payload: payload || {} }),
    updateDesktopLyrics: (payload: unknown) =>
      invoke('mineradio-desktop-lyrics-update', { payload: payload || {} }),
    onDesktopLyricsLockState: (cb: AnyFn) => on('mineradio-desktop-lyrics-lock-state', cb),
    onDesktopLyricsEnabledState: (cb: AnyFn) => on('mineradio-desktop-lyrics-enabled-state', cb),
    setWallpaperMode: (enabled: boolean, payload: unknown) =>
      invoke('mineradio-wallpaper-set-enabled', { enabled: !!enabled, payload: payload || {} }),
    updateWallpaperMode: (payload: unknown) =>
      invoke('mineradio-wallpaper-update', { payload: payload || {} }),
    onStateChange: (cb: AnyFn) => on('desktop-window-state', cb),
  };

  // ---- window.desktopOverlay（桌面歌词 / 壁纸覆盖层）----
  (window as any).desktopOverlay = {
    onLyricsState: (cb: AnyFn) => on('mineradio-desktop-lyrics-state', cb),
    onWallpaperState: (cb: AnyFn) => on('mineradio-wallpaper-state', cb),
    setLyricsDrag: (dragging: boolean) =>
      invoke('mineradio-desktop-lyrics-set-dragging', { dragging: !!dragging }),
    setLyricsPointerCapture: (active: boolean) =>
      invoke('mineradio-desktop-lyrics-set-pointer-capture', { active: !!active }),
    setLyricsHotBounds: (bounds: unknown) =>
      invoke('mineradio-desktop-lyrics-set-hot-bounds', { bounds: bounds || {} }),
    setLyricsLockState: (locked: boolean) =>
      invoke('mineradio-desktop-lyrics-set-lock-state', { locked: !!locked }),
    moveLyricsBy: (dx: number, dy: number) =>
      invoke('mineradio-desktop-lyrics-move-by', { dx: Number(dx) || 0, dy: Number(dy) || 0 }),
    closeLyrics: () =>
      invoke('mineradio-desktop-lyrics-set-enabled', { enabled: false, payload: {} }),
  };

  // 复刻 preload 的桌面外壳 class（CSS 依赖）。
  const markShell = () => {
    document.documentElement.classList.add('desktop-shell-root');
    document.body && document.body.classList.add('desktop-shell');
  };
  if (document.readyState === 'loading') {
    window.addEventListener('DOMContentLoaded', markShell);
  } else {
    markShell();
  }
}
