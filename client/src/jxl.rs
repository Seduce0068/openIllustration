use std::io::Cursor;
use wasm_bindgen::prelude::*;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, ImageData};

/// デコンプレッション bomb 対策。サーバーの取得上限 10 MiB に対して
/// 8000x8000 = 64M ピクセル（RGBA で 256 MB）を上限とする。
const MAX_DIMENSION: u32 = 8000;
const MAX_PIXELS: u64 = 64_000_000;

/// デコード前に寸法が安全かを判定する（WASM 非依存・テスト可能）。
fn dimensions_allowed(width: u64, height: u64) -> bool {
    width > 0
        && height > 0
        && width <= u64::from(MAX_DIMENSION)
        && height <= u64::from(MAX_DIMENSION)
        && width.saturating_mul(height) <= MAX_PIXELS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jxl_dimension_limits() {
        assert!(dimensions_allowed(1, 1));
        assert!(dimensions_allowed(8000, 8000));
        assert!(dimensions_allowed(1920, 1080));
        assert!(!dimensions_allowed(0, 100));
        assert!(!dimensions_allowed(100, 0));
        assert!(!dimensions_allowed(8001, 1));
        assert!(!dimensions_allowed(1, 8001));
        // 8000x8000 を超える面積（片方の辺は上限内）。
        assert!(!dimensions_allowed(8000, 8001));
        // saturating_mul でオーバーフローしても安全に拒否される。
        assert!(!dimensions_allowed(u64::MAX, u64::MAX));
    }
}

#[wasm_bindgen]
pub fn decode_jxl_to_canvas(jxl_data: &[u8], canvas: HtmlCanvasElement) -> Result<(), JsValue> {
    let context = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("Failed to get 2d context"))?
        .dyn_into::<CanvasRenderingContext2d>()?;

    let reader = Cursor::new(jxl_data);
    let image = jxl_oxide::JxlImage::read_with_defaults(reader)
        .map_err(|e| JsValue::from_str(&format!("JXL decode error: {:?}", e)))?;

    // レンダリング前にヘッダーの寸法を検証し、巨大なバッファ確保を防ぐ。
    let header_width = image.width();
    let header_height = image.height();
    if !dimensions_allowed(u64::from(header_width), u64::from(header_height)) {
        return Err(JsValue::from_str("JXL dimensions exceed the allowed limit"));
    }

    let render = image
        .render_frame(0)
        .map_err(|e| JsValue::from_str(&format!("JXL render error: {:?}", e)))?;

    let fb = render.image_all_channels();
    let width = fb.width();
    let height = fb.height();
    let channels = fb.channels();

    // レンダ結果も再検証（ヘッダーと異なる寸法が返された場合の防御）。
    if !dimensions_allowed(width as u64, height as u64) {
        return Err(JsValue::from_str(
            "JXL rendered dimensions exceed the allowed limit",
        ));
    }

    canvas.set_width(width as u32);
    canvas.set_height(height as u32);

    // 上限内であることを確認済みなので usize 計算でもオーバーフローしない。
    let mut rgba = Vec::with_capacity(width * height * 4);
    let buf = fb.buf();

    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * channels;

            if offset + 2 < buf.len() {
                rgba.push((buf[offset] * 255.0) as u8);
                rgba.push((buf[offset + 1] * 255.0) as u8);
                rgba.push((buf[offset + 2] * 255.0) as u8);
                rgba.push(255);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 255]);
            }
        }
    }

    let image_data = ImageData::new_with_u8_clamped_array_and_sh(
        wasm_bindgen::Clamped(&rgba),
        width as u32,
        height as u32,
    )?;

    context.put_image_data(&image_data, 0.0, 0.0)?;

    Ok(())
}
