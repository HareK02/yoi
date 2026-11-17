//! Emit the same Rust-derived authoring schema used by the Backend.
//! `deno task generate:profile-schema` uses this for native/WASM parity tests;
//! there is no checked-in or independently maintained editor field schema.
fn main() {
    print!("{}", manifest::profile_authoring_schema_source());
}
