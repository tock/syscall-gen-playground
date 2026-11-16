#[proc_macro_attribute]
pub fn syscall(
    _attr: proc_macro::TokenStream,
    _item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    // Clear out everything so we can experiment with syntax without needing to
    // make it compile.
    proc_macro::TokenStream::new()
}
