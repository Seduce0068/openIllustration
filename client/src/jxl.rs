use wasm_bindgen::prelude::*;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, ImageData};
use std::io::Cursor;

#[wasm_bindgen]
pub fn decode_jxl_to_canvas(
    jxl_data: &[u8],
    canvas: HtmlCanvasElement,
) -> Result<(), JsValue> {
    let context = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("Failed to get 2d context"))?
        .dyn_into::<CanvasRenderingContext2d>()?;

    let reader = Cursor::new(jxl_data);
    let image = jxl_oxide::JxlImage::from_reader(reader)
        .map_err(|e| JsValue::from_str(&format!("JXL decode error: {:?}", e)))?;
    
    let render = image.render_frame(0)
        .map_err(|e| JsValue::from_str(&format!("JXL render error: {:?}", e)))?;
    
    let fb = render.image();
    let width = fb.width() as u32;
    let height = fb.height() as u32;

    canvas.set_width(width);
    canvas.set_height(height);

    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    
    for y in 0..height as usize {
        for x in 0..width as usize {
            let offset = (y * width as usize + x) * 3;
            let buf = fb.buf();
            
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
        width,
        height,
    )?;

    context.put_image_data(&image_data, 0.0, 0.0)?;

    Ok(())
}
