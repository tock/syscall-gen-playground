//! Proc-macro attributes that generate a Tock `SyscallDriver` implementation
//! from a capsule's command/subscribe/allow declarations.
//!
//! Rust's attribute macros have no way to declare "helper" attributes the
//! way derive macros can (`#[proc_macro_derive(X, attributes(helper))]`):
//! any attribute written on an item is independently name-resolved by
//! rustc, even one nested inside an item that an outer attribute macro goes
//! on to consume and rewrite. So every attribute used here -- `syscall`,
//! `command`, `subscribe`, `allow_read_only`, `allow_read_write` -- has to be
//! a real, top-level `#[proc_macro_attribute]` that callers `use` directly;
//! none of them can live behind a `syscall::` module path, because
//! proc-macro crates aren't allowed to export modules at all, only
//! `#[proc_macro]`/`#[proc_macro_attribute]`/`#[proc_macro_derive]`
//! functions at the crate root. That restriction is also why the actual
//! parsing/codegen lives in the separate `macros-core` crate (a plain
//! library, so it has no such restriction): `gen-capsule` -- a plain binary
//! that produces a *complete*, ready-to-drop-into-Tock capsule file rather
//! than just the macro-expanded fragment -- links against it directly. See
//! `macros-core/src/bin/gen_capsule.rs`.
//!
//! `command` itself never does real work: `#[syscall]` strips every
//! `#[command(..)]` attribute from the methods it sees before it returns
//! its output, so `command`'s own macro body never runs on legitimate
//! input. It exists only so `#[command(0, SuccessU32, Failure)]` resolves
//! to *something* while `#[syscall]` is still looking at the unexpanded
//! impl block; if it ever does run for real, that means it was used outside
//! a `#[syscall]` impl, which is an error.

use std::env;
use std::fs;
use std::path::PathBuf;

use macros_core::{build_subscribe, build_syscall, ident_of_self_ty, SyscallArgs};
use proc_macro2::TokenStream;
use syn::{ItemEnum, ItemImpl};

/// Writes `tokens`, pretty-printed, to `generated/<name>.rs` next to the
/// `Cargo.toml` of whichever crate invoked the macro, so the expansion can
/// be read without reaching for `cargo expand`. Best-effort: a problem
/// writing the file never fails the build.
fn dump_generated(name: &str, tokens: &TokenStream) {
    let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") else {
        return;
    };
    let dir = PathBuf::from(manifest_dir).join("generated");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = fs::write(
        dir.join(format!("{name}.rs")),
        macros_core::pretty_print(tokens),
    );
}

/// Marker attribute for `#[command(<num>, <SuccessVariant>, Failure)]`.
///
/// See the module docs: `#[syscall]` always removes this attribute before
/// it would be expanded on its own, so reaching this function body means
/// `#[command(..)]` was used on a method outside a `#[syscall]` impl.
#[proc_macro_attribute]
pub fn command(
    _attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let item = TokenStream::from(item);
    syn::Error::new_spanned(
        &item,
        "#[command(..)] only has meaning on a method inside an impl block \
         annotated with #[syscall]",
    )
    .to_compile_error()
    .into()
}

#[proc_macro_attribute]
pub fn syscall(
    attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let args = syn::parse_macro_input!(attr as SyscallArgs);
    let impl_block = syn::parse_macro_input!(item as ItemImpl);
    let name = ident_of_self_ty(&impl_block.self_ty);

    match build_syscall(args, impl_block) {
        Ok(tokens) => {
            dump_generated(&format!("{name}_syscall"), &tokens);
            tokens.into()
        }
        Err(e) => e.to_compile_error().into(),
    }
}

/// `#[subscribe]` on a fieldless-in-spirit enum that uses struct-variant
/// syntax purely as a DSL for documenting each upcall's argument names and
/// types; the enum itself is never emitted; it only has to parse. Generates
/// one `UPCALL_<NAME>: usize` constant per variant.
#[proc_macro_attribute]
pub fn subscribe(
    _attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let item = syn::parse_macro_input!(item as ItemEnum);
    let name = item.ident.to_string();

    match build_subscribe(&item) {
        Ok(tokens) => {
            dump_generated(&format!("{name}_subscribe"), &tokens);
            tokens.into()
        }
        Err(e) => e.to_compile_error().into(),
    }
}

/// `#[allow_read_only]` on a declarative, always-empty-for-now enum.
///
/// Mirrors a Tock capsule's read-only allow buffers once those exist as a
/// DSL; for now this only validates the syntax and generates nothing, same
/// as `#[allow_read_write]`.
#[proc_macro_attribute]
pub fn allow_read_only(
    _attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    if let Err(e) = syn::parse::<ItemEnum>(item) {
        return e.to_compile_error().into();
    }
    proc_macro::TokenStream::new()
}

/// `#[allow_read_write]`: see `#[allow_read_only]`.
#[proc_macro_attribute]
pub fn allow_read_write(
    _attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    if let Err(e) = syn::parse::<ItemEnum>(item) {
        return e.to_compile_error().into();
    }
    proc_macro::TokenStream::new()
}
