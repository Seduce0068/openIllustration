use wasm_bindgen::prelude::*;
use sha2::{Sha256, Digest};

#[wasm_bindgen]
pub fn verify_checksum(data: &[u8], expected_checksum: &str) -> bool {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let actual_checksum = hex::encode(result);
    
    actual_checksum == expected_checksum
}
