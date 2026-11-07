//! Raw WASM parity fixture. No allocator pointers, JavaScript packages or host imports.
#[path = "../tests/common/mod.rs"]
mod common;

#[unsafe(no_mangle)]
pub extern "C" fn parity_len() -> u32 {
    (common::objects().len() * 8) as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn parity_word(index: u32) -> u32 {
    common::parity_word(index)
}
