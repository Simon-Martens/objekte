//! Encode selected record text for a terminal clipboard request.

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(super) fn osc52(text: &str) -> String {
    let mut encoded = String::new();
    for chunk in text.as_bytes().chunks(3) {
        let value = chunk.iter().enumerate().fold(0u32, |value, (index, byte)| {
            value | (u32::from(*byte) << (16 - 8 * index))
        });
        for index in 0..4 {
            encoded.push(if index <= chunk.len() {
                BASE64[((value >> (18 - index * 6)) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    format!("\x1b]52;c;{encoded}\x07")
}

#[cfg(test)]
#[path = "clipboard.test.rs"]
mod tests;
