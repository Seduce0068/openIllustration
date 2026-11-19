use std::io::Cursor;
use wasm_bindgen::prelude::*;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, ImageData};

/// デコンプレッション bomb 対策。サーバーの取得上限 10 MiB に対して
/// 8000x8000 = 64M ピクセル（RGBA で 256 MB）を上限とする。
/// 入力バイト数の 10 MiB 上限はサーバー側の取得・検証で担保されるため、
/// クライアント側では重複チェックせず、デコード前後の寸法のみを検証する。
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

/// インターリーブされたサンプルを RGBA に変換する。
/// channels=1 はグレー、2 はグレー＋アルファ、3 は RGB、4 以上は RGBA として扱う。
fn sample_to_rgba(buf: &[f32], offset: usize, channels: usize) -> [u8; 4] {
    let to_u8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
    let at = |i: usize| buf.get(offset + i).copied().unwrap_or(0.0);
    match channels {
        1 => {
            let g = to_u8(at(0));
            [g, g, g, 255]
        }
        2 => {
            let g = to_u8(at(0));
            [g, g, g, to_u8(at(1))]
        }
        3 => [to_u8(at(0)), to_u8(at(1)), to_u8(at(2)), 255],
        _ => [to_u8(at(0)), to_u8(at(1)), to_u8(at(2)), to_u8(at(3))],
    }
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

    /// proptest は dev-dependencies に無いため、決定論的な疑似乱数ループで
    /// プロパティを検証する（新規依存クレートは追加しない）。
    #[test]
    fn jxl_dimension_fuzz_property() {
        // xorshift64* による再現可能なシード。flaky にならない固定値。
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545_F491_4F6C_DD1D)
        };

        let max_dim = u64::from(MAX_DIMENSION);
        // 0 と上限超過を必ず混ぜるため、8000 を超える範囲で生成する。
        let sample_range = max_dim + 1000;
        for _ in 0..10_000 {
            let w = next() % sample_range;
            let h = next() % sample_range;

            // ガードの判定が上限定義と整合することを再確認する。
            let expected =
                w > 0 && h > 0 && w <= max_dim && h <= max_dim && w.saturating_mul(h) <= MAX_PIXELS;
            assert_eq!(
                dimensions_allowed(w, h),
                expected,
                "guard mismatch for {w}x{h}"
            );
        }
    }

    #[test]
    fn jxl_channel_conversion() {
        let gray = [0.5];
        assert_eq!(sample_to_rgba(&gray, 0, 1), [127, 127, 127, 255]);
        let gray_alpha = [0.5, 0.25];
        assert_eq!(sample_to_rgba(&gray_alpha, 0, 2), [127, 127, 127, 63]);
        let rgb = [1.0, 0.0, 0.5];
        assert_eq!(sample_to_rgba(&rgb, 0, 3), [255, 0, 127, 255]);
        let rgba = [1.0, 0.0, 0.5, 0.8];
        assert_eq!(sample_to_rgba(&rgba, 0, 4), [255, 0, 127, 204]);
        // extra channels を含む 5 チャンネルでも先頭 4 つを使う。
        let extra = [1.0, 0.0, 0.5, 0.8, 0.1];
        assert_eq!(sample_to_rgba(&extra, 0, 5), [255, 0, 127, 204]);
        // オフセット指定と範囲外は 0 で埋める（パニックしない）。
        let multi = [0.1, 0.2, 0.3, 0.4, 0.5];
        assert_eq!(sample_to_rgba(&multi, 2, 4), [76, 102, 127, 0]);
        assert_eq!(sample_to_rgba(&[], 0, 4), [0, 0, 0, 0]);
        // 値は 0..1 にクランプする。
        let over = [2.0, -1.0, 0.5, 1.0];
        assert_eq!(sample_to_rgba(&over, 0, 4), [255, 0, 127, 255]);
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
            rgba.extend_from_slice(&sample_to_rgba(buf, offset, channels));
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
