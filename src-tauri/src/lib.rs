// Mineradio — Tauri 2 + Rust 入口
//
// setup 中探测/确定端口、启动内嵌 axum 后端（替换 server.js）。
// 开发期：后端监听固定端口（rsbuild 代理 /api 到此），窗口仍由 devUrl 加载。
// 生产期：后端监听空闲端口并把主窗口指向 http://127.0.0.1:PORT/。

pub mod commands;
pub mod login;
pub mod server;

use tauri::{Emitter, Manager};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        if let Some(action) = commands::hotkey_action(&shortcut.to_string()) {
                            let _ = app.emit("mineradio-global-hotkey", serde_json::json!({ "action": action }));
                        }
                    }
                })
                .build(),
        )
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
            // 随机空闲高位端口；axum 同源服务前端静态资源(嵌入) + /api，dev/prod 一致。
            let listener = server::bind_listener()?;
            let port = listener.local_addr()?.port();
            let base = format!("http://127.0.0.1:{port}");
            commands::set_frontend_base(base.clone());

            // 网易云 / QQ cookie 持久化路径（应用数据目录）。
            if let Ok(dir) = app.path().app_data_dir() {
                let _ = std::fs::create_dir_all(&dir);
                server::netease::cookie_store::init(dir.join("netease.cookie"));
                server::qq::init(dir.join("qq.cookie"));
            }

            // 端口已 LISTEN，先起后端再建窗口（连接不丢）。
            tauri::async_runtime::spawn(async move {
                if let Err(e) = server::serve(listener).await {
                    tracing::error!("后端启动失败: {e}");
                }
            });

            // 主窗口直接加载 axum（无固定端口、无 tauri:// 闪屏）。
            let url: tauri::Url = format!("{base}/").parse()?;
            let mut builder = tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External(url))
                .title("Mineradio")
                .inner_size(1280.0, 720.0)
                .min_inner_size(960.0, 540.0)
                .resizable(true)
                // 窗口底色设为应用深色，避免 macOS 圆角处露出白底（白角）。
                .background_color(tauri::webview::Color(6, 7, 13, 255))
                .center();
            #[cfg(target_os = "macos")]
            {
                builder = builder
                    .title_bar_style(tauri::TitleBarStyle::Overlay)
                    .hidden_title(true);
            }
            #[cfg(not(target_os = "macos"))]
            {
                // Windows/Linux：无边框，应用自绘标题栏与窗口控件。
                builder = builder.decorations(false);
            }
            builder.build()?;

            tracing::info!("Mineradio 启动 (port={port})");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("运行 Mineradio 时发生错误");
}
