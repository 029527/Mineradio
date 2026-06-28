// Mineradio — Tauri 2 + Rust 入口
//
// setup 中探测/确定端口、启动内嵌 axum 后端（替换 server.js）。
// 开发期：后端监听固定端口（rsbuild 代理 /api 到此），窗口仍由 devUrl 加载。
// 生产期：后端监听空闲端口并把主窗口指向 http://127.0.0.1:PORT/。

pub mod commands;
pub mod server;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::desktop_window_minimize,
            commands::desktop_window_toggle_maximize,
            commands::desktop_window_toggle_fullscreen,
            commands::desktop_window_exit_fullscreen_windowed,
            commands::desktop_window_close,
            commands::desktop_window_get_state,
            commands::restart_app,
            commands::desktop_lyrics_set_enabled,
            commands::desktop_lyrics_update,
            commands::desktop_lyrics_set_dragging,
            commands::desktop_lyrics_set_pointer_capture,
            commands::desktop_lyrics_set_hot_bounds,
            commands::desktop_lyrics_set_lock_state,
            commands::desktop_lyrics_move_by,
            commands::wallpaper_set_enabled,
            commands::wallpaper_update,
            commands::hotkeys_configure_global,
            commands::export_json_file,
            commands::import_json_file,
            commands::netease_music_open_login,
            commands::netease_music_clear_login,
            commands::qq_music_open_login,
            commands::qq_music_clear_login,
            commands::open_update_installer,
        ])
        .setup(|app| {
            let is_dev = tauri::is_dev();
            let port = if is_dev {
                std::env::var("MINERADIO_DEV_API_PORT")
                    .ok()
                    .and_then(|v| v.parse::<u16>().ok())
                    .unwrap_or(3000)
            } else {
                server::find_free_port().unwrap_or(3000)
            };

            // 覆盖层窗口加载基址：开发期走 rsbuild(1420)，生产期走 axum。
            let frontend_base = if is_dev {
                "http://localhost:1420".to_string()
            } else {
                format!("http://127.0.0.1:{port}")
            };
            commands::set_frontend_base(frontend_base);

            tauri::async_runtime::spawn(async move {
                if let Err(e) = server::serve(port).await {
                    tracing::error!("后端启动失败: {e}");
                }
            });

            if !is_dev {
                if let Some(win) = app.get_webview_window("main") {
                    if let Ok(url) = format!("http://127.0.0.1:{port}/").parse() {
                        let _ = win.navigate(url);
                    }
                }
            }

            tracing::info!("Mineradio 启动 (port={port}, dev={is_dev})");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("运行 Mineradio 时发生错误");
}
