//! 扫码登录二维码生成（替换 NeteaseCloudMusicApi 的 QRCode.toDataURL）。
//! 输出 PNG 的 data URL，供前端 <img src> 直接渲染。

use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::{ImageEncoder, Luma};
use qrcode::{Color, QrCode};

/// 生成二维码 PNG 的 data URL。失败返回 None。
pub fn data_url(content: &str) -> Option<String> {
    let code = QrCode::new(content.as_bytes()).ok()?;
    let width = code.width();
    let colors = code.to_colors();

    const SCALE: usize = 8;
    const QUIET: usize = 4;
    let img_dim = ((width + QUIET * 2) * SCALE) as u32;

    let mut img = image::GrayImage::from_pixel(img_dim, img_dim, Luma([255u8]));
    for y in 0..width {
        for x in 0..width {
            if matches!(colors[y * width + x], Color::Dark) {
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        let px = ((x + QUIET) * SCALE + dx) as u32;
                        let py = ((y + QUIET) * SCALE + dy) as u32;
                        img.put_pixel(px, py, Luma([0u8]));
                    }
                }
            }
        }
    }

    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&img, img_dim, img_dim, image::ExtendedColorType::L8)
        .ok()?;
    Some(format!("data:image/png;base64,{}", STANDARD.encode(png)))
}
