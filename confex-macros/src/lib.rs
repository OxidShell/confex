use proc_macro::TokenStream;

/// Derives `ConfigSchema` for a struct (Phase 1 — not yet implemented).
///
/// Reads `#[config(app_name = "...", version = N)]` at struct level and
/// `#[config(merge = "...", encrypted)]` at field level.
#[proc_macro_derive(ConfigSchema, attributes(config))]
pub fn derive_config_schema(input: TokenStream) -> TokenStream {
    let _ = input;
    TokenStream::new()
}
