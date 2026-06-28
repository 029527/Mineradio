// Tauri 桥接层：在 Tauri webview 内重建 Electron preload 暴露的
// window.desktopWindow / window.desktopOverlay，底层走 window.__TAURI__
// （tauri.conf app.withGlobalTauri = true 注入）。
//
// 约定：invoke 命令名用 snake_case（与 src-tauri/commands.rs 一一对应），
// 参数键也用 snake_case（匹配 Rust 形参）；listen 事件名沿用连字符字符串。
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
    minimize: () => invoke('desktop_window_minimize'),
    toggleMaximize: () => invoke('desktop_window_toggle_maximize'),
    toggleFullscreen: () => invoke('desktop_window_toggle_fullscreen'),
    exitFullscreenWindowed: () => invoke('desktop_window_exit_fullscreen_windowed'),
    getState: () => invoke('desktop_window_get_state'),
    close: () => invoke('desktop_window_close'),
    // 不暴露 openNeteaseMusicLogin：网易云走应用内扫码(/api/login/qr/*)，体验更顺、更稳。
    // (前端据 typeof openNeteaseMusicLogin === 'function' 决定二维码 vs 网页登录)
    clearNeteaseMusicLogin: () => invoke('netease_music_clear_login'),
    openQQMusicLogin: () => invoke('qq_music_open_login'),
    clearQQMusicLogin: () => invoke('qq_music_clear_login'),
    openUpdateInstaller: (filePath: string) =>
      invoke('open_update_installer', { file_path: filePath }),
    restartApp: () => invoke('restart_app'),
    configureGlobalHotkeys: (bindings: unknown) =>
      invoke('hotkeys_configure_global', { bindings: bindings || [] }),
    exportJsonFile: (payload: unknown) => invoke('export_json_file', { payload: payload || {} }),
    importJsonFile: () => invoke('import_json_file'),
    onGlobalHotkey: (cb: AnyFn) => on('mineradio-global-hotkey', cb),
    setDesktopLyricsEnabled: (enabled: boolean, payload: unknown) =>
      invoke('desktop_lyrics_set_enabled', { enabled: !!enabled, payload: payload || {} }),
    updateDesktopLyrics: (payload: unknown) =>
      invoke('desktop_lyrics_update', { payload: payload || {} }),
    onDesktopLyricsLockState: (cb: AnyFn) => on('mineradio-desktop-lyrics-lock-state', cb),
    onDesktopLyricsEnabledState: (cb: AnyFn) => on('mineradio-desktop-lyrics-enabled-state', cb),
    setWallpaperMode: (enabled: boolean, payload: unknown) =>
      invoke('wallpaper_set_enabled', { enabled: !!enabled, payload: payload || {} }),
    updateWallpaperMode: (payload: unknown) => invoke('wallpaper_update', { payload: payload || {} }),
    onStateChange: (cb: AnyFn) => on('desktop-window-state', cb),
  };

  // ---- window.desktopOverlay（桌面歌词 / 壁纸覆盖层）----
  (window as any).desktopOverlay = {
    onLyricsState: (cb: AnyFn) => on('mineradio-desktop-lyrics-state', cb),
    onWallpaperState: (cb: AnyFn) => on('mineradio-wallpaper-state', cb),
    setLyricsDrag: (dragging: boolean) =>
      invoke('desktop_lyrics_set_dragging', { dragging: !!dragging }),
    setLyricsPointerCapture: (active: boolean) =>
      invoke('desktop_lyrics_set_pointer_capture', { active: !!active }),
    setLyricsHotBounds: (bounds: unknown) =>
      invoke('desktop_lyrics_set_hot_bounds', { bounds: bounds || {} }),
    setLyricsLockState: (locked: boolean) =>
      invoke('desktop_lyrics_set_lock_state', { locked: !!locked }),
    moveLyricsBy: (dx: number, dy: number) =>
      invoke('desktop_lyrics_move_by', { dx: Number(dx) || 0, dy: Number(dy) || 0 }),
    closeLyrics: () => invoke('desktop_lyrics_set_enabled', { enabled: false, payload: {} }),
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

  // 无边框窗口：让应用自绘标题栏可拖动/双击最大化（替代 Electron 的 -webkit-app-region）。
  const tauriWindow = (window as any).__TAURI__?.window;
  if (tauriWindow?.getCurrentWindow) {
    const appWindow = tauriWindow.getCurrentWindow();

    // macOS Overlay 标题栏：左上角有原生红绿灯。
    // 1) 给应用顶栏留出左侧空间避免遮挡；2) 隐藏应用自绘的 min/max/close(与红绿灯重复)，
    //    保留 DIY/更新/引导按钮。(Windows 无边框、无红绿灯，仍用应用自绘控件)
    const isMac = /Mac/i.test(navigator.platform) || /Macintosh/i.test(navigator.userAgent);
    if (isMac) {
      const style = document.createElement('style');
      style.textContent =
        'body.desktop-shell #desktop-titlebar{padding-left:84px}' +
        'body.desktop-shell .desktop-window-btn{display:none!important}';
      (document.head || document.documentElement).appendChild(style);
    }

    const NO_DRAG = '.desktop-window-controls, button, input, a, select, textarea, [data-window-action], .desktop-mode-btn, #update-entry, #visual-guide-btn';
    // 基于「可见顶栏的矩形范围」判断拖拽区：整条顶栏(含红绿灯那一排，红绿灯本身由系统处理)
    // 只要不是交互控件都可拖动。避免 pointer-events:none 容器 / 左侧留白导致命中失败。
    const inDragRegion = (e: PointerEvent | MouseEvent): boolean => {
      const tb = document.getElementById('desktop-titlebar');
      if (!tb) return false;
      const cs = getComputedStyle(tb);
      if (cs.display === 'none' || cs.visibility === 'hidden') return false;
      const r = tb.getBoundingClientRect();
      if (r.height === 0) return false;
      if (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom) return false;
      const el = e.target as HTMLElement | null;
      if (el && el.closest && el.closest(NO_DRAG)) return false;
      return true;
    };
    window.addEventListener('pointerdown', (e) => {
      if (e.button !== 0) return;
      if (inDragRegion(e)) {
        appWindow.startDragging?.().catch(() => {});
      }
    });
    window.addEventListener('dblclick', (e) => {
      if (inDragRegion(e)) {
        invoke('desktop_window_toggle_maximize').catch(() => {});
      }
    });
  }
}
