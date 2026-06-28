// Mineradio — Tauri 2 + Rust 入口
//
// 脚手架阶段：仅建立主窗口并跑通 `tauri dev`。
// 后续阶段会在 setup 中探测空闲端口、启动内嵌 axum 后端（替换 server.js），
// 并把主窗口指向 http://127.0.0.1:PORT/。

pub mod server;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    tauri::Builder::default()
        .setup(|_app| {
            tracing::info!("Mineradio 启动");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("运行 Mineradio 时发生错误");
}
