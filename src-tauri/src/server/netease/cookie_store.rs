//! 网易云登录态（MUSIC_U）的进程内存储。
//! 持久化（.cookie 文件读写）将在登录阶段接入；当前默认匿名态。

use std::sync::RwLock;

static MUSIC_U: RwLock<String> = RwLock::new(String::new());

/// 读取当前 MUSIC_U（匿名时为空）。
pub fn music_u() -> Option<String> {
    let v = MUSIC_U.read().unwrap();
    if v.is_empty() {
        None
    } else {
        Some(v.clone())
    }
}

/// 设置 MUSIC_U（登录成功后调用）。
pub fn set_music_u(value: impl Into<String>) {
    *MUSIC_U.write().unwrap() = value.into();
}

/// 是否已登录。
pub fn is_logged_in() -> bool {
    !MUSIC_U.read().unwrap().is_empty()
}
