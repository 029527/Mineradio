//! Tauri 命令（替换 desktop/main.js 的 ipcMain 处理器）。
//!
//! 命令名采用 snake_case（与 frontend/src/bridge 一一对应）；事件名沿用原
//! 连字符字符串。窗口控制 / 桌面歌词 / 壁纸为真实实现；热键 / 对话框 / 登录 /
//! 更新先返回优雅占位（后续阶段接入对应插件）。

use std::sync::OnceLock;

use serde_json::{json, Value};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

// 前端基址：开发期 rsbuild(1420)，生产期 axum 端口。覆盖层窗口据此加载页面。
static FRONTEND_BASE: OnceLock<String> = OnceLock::new();

pub fn set_frontend_base(url: String) {
    let _ = FRONTEND_BASE.set(url);
}

fn overlay_url(page: &str) -> WebviewUrl {
    let base = FRONTEND_BASE
        .get()
        .cloned()
        .unwrap_or_else(|| "http://127.0.0.1:3000".into());
    WebviewUrl::External(format!("{base}/{page}").parse().expect("覆盖层 URL 非法"))
}

// ---------------- 窗口控制 ----------------

#[tauri::command]
pub fn desktop_window_minimize(window: WebviewWindow) {
    let _ = window.minimize();
}

#[tauri::command]
pub fn desktop_window_toggle_maximize(window: WebviewWindow) {
    // 对齐 main.js：最大化按钮即切换原生全屏。
    let fs = window.is_fullscreen().unwrap_or(false);
    let _ = window.set_fullscreen(!fs);
}

#[tauri::command]
pub fn desktop_window_toggle_fullscreen(window: WebviewWindow) {
    let fs = window.is_fullscreen().unwrap_or(false);
    let _ = window.set_fullscreen(!fs);
}

#[tauri::command]
pub fn desktop_window_exit_fullscreen_windowed(window: WebviewWindow) {
    let _ = window.set_fullscreen(false);
}

#[tauri::command]
pub fn desktop_window_close(window: WebviewWindow) {
    let _ = window.close();
}

#[tauri::command]
pub fn desktop_window_get_state(window: WebviewWindow) -> Value {
    window_state(&window)
}

fn window_state(window: &WebviewWindow) -> Value {
    let is_fullscreen = window.is_fullscreen().unwrap_or(false);
    json!({
        "isMaximized": window.is_maximized().unwrap_or(false),
        "isNativeFullScreen": is_fullscreen,
        "isHtmlFullScreen": false,
        "isWindowFullScreen": false,
        "isFullScreen": is_fullscreen,
        "isMinimized": window.is_minimized().unwrap_or(false),
        "isVisible": window.is_visible().unwrap_or(true),
        "isFocused": window.is_focused().unwrap_or(false),
        // 多显示器布局字段：首版给安全默认值（前端据此防御式取用）。
        "isPrimaryDisplay": true,
        "hasDisplayOnLeft": false,
        "hasDisplayOnRight": false,
        "displayBounds": Value::Null,
    })
}

/// 主窗口状态变更时广播（窗口事件回调里调用）。
pub fn emit_window_state(window: &WebviewWindow) {
    let state = window_state(window);
    let _ = window.emit("desktop-window-state", state);
}

// ---------------- 应用生命周期 ----------------

#[tauri::command]
pub fn restart_app(app: AppHandle) -> Value {
    app.restart();
}

// ---------------- 桌面歌词覆盖层 ----------------

const LYRICS_LABEL: &str = "desktop-lyrics";

fn ensure_lyrics_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(w) = app.get_webview_window(LYRICS_LABEL) {
        return Ok(w);
    }
    let win = WebviewWindowBuilder::new(app, LYRICS_LABEL, overlay_url("desktop-lyrics.html"))
        .title("Mineradio Desktop Lyrics")
        .inner_size(920.0, 190.0)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .focused(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .build()?;
    Ok(win)
}

#[tauri::command]
pub fn desktop_lyrics_set_enabled(app: AppHandle, enabled: bool, payload: Value) -> Value {
    if enabled {
        match ensure_lyrics_window(&app) {
            Ok(win) => {
                let _ = win.emit("mineradio-desktop-lyrics-state", &payload);
                let _ = app.emit("mineradio-desktop-lyrics-enabled-state", json!({ "enabled": true }));
                json!({ "ok": true })
            }
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        }
    } else {
        if let Some(win) = app.get_webview_window(LYRICS_LABEL) {
            let _ = win.close();
        }
        let _ = app.emit("mineradio-desktop-lyrics-enabled-state", json!({ "enabled": false }));
        json!({ "ok": true })
    }
}

#[tauri::command]
pub fn desktop_lyrics_update(app: AppHandle, payload: Value) -> Value {
    if let Some(win) = app.get_webview_window(LYRICS_LABEL) {
        let _ = win.emit("mineradio-desktop-lyrics-state", &payload);
    }
    json!({ "ok": true })
}

#[tauri::command]
pub fn desktop_lyrics_set_dragging(_dragging: bool) -> Value {
    // 拖动经 desktop_lyrics_move_by 实现，这里无需额外处理。
    json!({ "ok": true })
}

#[tauri::command]
pub fn desktop_lyrics_set_pointer_capture(app: AppHandle, active: bool) -> Value {
    // macOS 交互/穿透切换：指针进入热区 → 可交互；离开 → 穿透。
    if let Some(win) = app.get_webview_window(LYRICS_LABEL) {
        let _ = win.set_ignore_cursor_events(!active);
    }
    json!({ "ok": true })
}

#[tauri::command]
pub fn desktop_lyrics_set_hot_bounds(_bounds: Value) -> Value {
    // 热区由前端 pointer_capture 驱动（macOS）；Windows 鼠标轮询为后续平台门控。
    json!({ "ok": true })
}

#[tauri::command]
pub fn desktop_lyrics_set_lock_state(app: AppHandle, locked: bool) -> Value {
    if let Some(win) = app.get_webview_window(LYRICS_LABEL) {
        let _ = win.set_ignore_cursor_events(locked);
    }
    let _ = app.emit("mineradio-desktop-lyrics-lock-state", json!({ "locked": locked }));
    json!({ "ok": true })
}

#[tauri::command]
pub fn desktop_lyrics_move_by(app: AppHandle, dx: f64, dy: f64) -> Value {
    if let Some(win) = app.get_webview_window(LYRICS_LABEL) {
        if let Ok(pos) = win.outer_position() {
            let _ = win.set_position(PhysicalPosition::new(pos.x + dx as i32, pos.y + dy as i32));
        }
    }
    json!({ "ok": true })
}

// ---------------- 壁纸覆盖层 ----------------

const WALLPAPER_LABEL: &str = "wallpaper";

#[tauri::command]
pub fn wallpaper_set_enabled(app: AppHandle, enabled: bool, payload: Value) -> Value {
    if enabled {
        if app.get_webview_window(WALLPAPER_LABEL).is_none() {
            let built = WebviewWindowBuilder::new(&app, WALLPAPER_LABEL, overlay_url("wallpaper.html"))
                .title("Mineradio Wallpaper")
                .decorations(false)
                .skip_taskbar(true)
                .maximized(true)
                .build();
            if let Err(e) = built {
                return json!({ "ok": false, "error": e.to_string() });
            }
            // Windows 专有：贴到桌面 WorkerW（SetParent）。后续平台门控接入。
            #[cfg(windows)]
            {
                // TODO: windows-rs SetParent/SetWindowPos 贴桌面
            }
        }
        if let Some(win) = app.get_webview_window(WALLPAPER_LABEL) {
            let _ = win.emit("mineradio-wallpaper-state", &payload);
        }
        json!({ "ok": true })
    } else {
        if let Some(win) = app.get_webview_window(WALLPAPER_LABEL) {
            let _ = win.close();
        }
        json!({ "ok": true })
    }
}

#[tauri::command]
pub fn wallpaper_update(app: AppHandle, payload: Value) -> Value {
    if let Some(win) = app.get_webview_window(WALLPAPER_LABEL) {
        let _ = win.emit("mineradio-wallpaper-state", &payload);
    }
    json!({ "ok": true })
}

// ---------------- 占位命令（后续阶段接入插件 / 登录）----------------

#[tauri::command]
pub fn hotkeys_configure_global(_bindings: Value) -> Value {
    // TODO(阶段#6): tauri-plugin-global-shortcut 注册
    json!({ "ok": false, "error": "NOT_IMPLEMENTED" })
}

#[tauri::command]
pub fn export_json_file(_payload: Value) -> Value {
    // TODO(阶段#6): tauri-plugin-dialog + fs 写出
    json!({ "ok": false, "error": "NOT_IMPLEMENTED" })
}

#[tauri::command]
pub fn import_json_file() -> Value {
    // TODO(阶段#6): tauri-plugin-dialog + fs 读入
    json!({ "ok": false, "error": "NOT_IMPLEMENTED" })
}

#[tauri::command]
pub fn netease_music_open_login() -> Value {
    json!({ "ok": false, "error": "NOT_IMPLEMENTED" })
}

#[tauri::command]
pub fn netease_music_clear_login() -> Value {
    json!({ "ok": false, "error": "NOT_IMPLEMENTED" })
}

#[tauri::command]
pub fn qq_music_open_login() -> Value {
    json!({ "ok": false, "error": "NOT_IMPLEMENTED" })
}

#[tauri::command]
pub fn qq_music_clear_login() -> Value {
    json!({ "ok": false, "error": "NOT_IMPLEMENTED" })
}

#[tauri::command]
pub fn open_update_installer(_file_path: String) -> Value {
    json!({ "ok": false, "error": "NOT_IMPLEMENTED" })
}
