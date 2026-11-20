//! Produces a complete, ready-to-drop-into-Tock capsule file from a source
//! file written against the `syscall`/`command`/`subscribe`/
//! `allow_read_only`/`allow_read_write` attributes.
//!
//! A proc-macro only ever receives the one item it's attached to, so by
//! construction it can't hand back "the rest of the file, untouched" --
//! only the item it rewrote. To get a full capsule file (license header,
//! doc comments, unrelated impls, all byte-for-byte as written) with just
//! the macro-annotated pieces resolved, this works on the file's *source
//! text* directly: it parses it to find each annotated item's exact byte
//! range (via span info), replaces just those ranges with the same
//! generated code the proc-macros would produce, and leaves every other
//! byte of the file alone -- comments included, which the proc-macro path
//! can't preserve since plain (non-doc) comments aren't part of the token
//! stream a proc-macro ever sees.
//!
//! Usage: `gen-capsule <input.rs> [output.rs]` (default output:
//! `generated/<input file name>` next to the input file's crate).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::Span;
use syn::spanned::Spanned;
use syn::{Attribute, Item, ItemImpl};

struct Edit {
    start: usize,
    end: usize,
    replacement: String,
}

fn main() {
    let mut args = env::args_os().skip(1);
    let Some(input) = args.next() else {
        eprintln!("usage: gen-capsule <input.rs> [output.rs]");
        std::process::exit(2);
    };
    let input = PathBuf::from(input);
    let output = match args.next() {
        Some(path) => PathBuf::from(path),
        None => default_output_path(&input),
    };

    let src = fs::read_to_string(&input).unwrap_or_else(|e| {
        eprintln!("error: couldn't read {}: {e}", input.display());
        std::process::exit(1);
    });

    let out = match splice(&src) {
        Ok(out) => out,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    if let Some(parent) = output.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(&output, out).unwrap_or_else(|e| {
        eprintln!("error: couldn't write {}: {e}", output.display());
        std::process::exit(1);
    });
    eprintln!("wrote {}", output.display());
}

fn default_output_path(input: &Path) -> PathBuf {
    let file_name = input.file_name().unwrap_or_else(|| "capsule.rs".as_ref());
    PathBuf::from("generated").join(file_name)
}

/// Finds the byte range of `span` in the source text that was parsed to
/// produce it. `syn`/`proc-macro2` only track this when built with the
/// `span-locations` feature (on here), and only for a plain (non-macro)
/// parse like this tool's -- which is exactly our situation.
fn byte_range(span: Span) -> std::ops::Range<usize> {
    span.byte_range()
}

/// Widens `start..end` to cover the whole line(s) it's on, swallowing
/// leading indentation and the trailing newline, so deleting it doesn't
/// leave a blank line behind. Leaves the range alone if there's other,
/// non-whitespace content sharing either edge's line.
fn widen_to_whole_lines(src: &str, start: usize, end: usize) -> (usize, usize) {
    let line_start = src[..start].rfind('\n').map_or(0, |i| i + 1);
    let new_start = if src[line_start..start].chars().all(char::is_whitespace) {
        line_start
    } else {
        start
    };

    let new_end = match src[end..].find('\n') {
        Some(rel) if src[end..end + rel].chars().all(char::is_whitespace) => end + rel + 1,
        _ => end,
    };

    (new_start, new_end)
}

fn delete_span(src: &str, span: Span) -> syn::Result<Edit> {
    let range = byte_range(span);
    let (start, end) = widen_to_whole_lines(src, range.start, range.end);
    Ok(Edit {
        start,
        end,
        replacement: String::new(),
    })
}

fn replace_span(src: &str, span: Span, replacement: String) -> syn::Result<Edit> {
    let range = byte_range(span);
    let (start, end) = widen_to_whole_lines(src, range.start, range.end);
    Ok(Edit {
        start,
        end,
        replacement,
    })
}

fn insert_after_span(span: Span, text: String) -> syn::Result<Edit> {
    let range = byte_range(span);
    Ok(Edit {
        start: range.end,
        end: range.end,
        replacement: text,
    })
}

fn find_attr<'a>(attrs: &'a [Attribute], name: &str) -> Option<&'a Attribute> {
    attrs.iter().find(|a| a.path().is_ident(name))
}

fn is_macros_use(item_use: &syn::ItemUse) -> bool {
    matches!(&item_use.tree, syn::UseTree::Path(p) if p.ident == "macros")
}

fn syscall_edits(src: &str, imp: &ItemImpl) -> syn::Result<Vec<Edit>> {
    let mut edits = Vec::new();

    let syscall_attr = find_attr(&imp.attrs, "syscall")
        .expect("caller already checked this impl has #[syscall(..)]");
    let args: macros_core::SyscallArgs = match &syscall_attr.meta {
        syn::Meta::List(list) => syn::parse2(list.tokens.clone())?,
        syn::Meta::Path(_) => macros_core::SyscallArgs { grant: None },
        syn::Meta::NameValue(nv) => {
            return Err(syn::Error::new(nv.span(), "expected `#[syscall]` or `#[syscall(grant = ..)]`"))
        }
    };
    edits.push(delete_span(src, syscall_attr.span())?);

    // Work on a clone: we only want the *spans* of the `#[command(..)]`
    // attributes (which survive cloning unchanged, since they refer to
    // positions in the original source text, not to this in-memory tree),
    // not a rewritten impl block -- the original source text for every
    // method is kept byte-for-byte except for deleting those attributes.
    let mut for_extraction = imp.clone();
    let found = macros_core::extract_commands(&mut for_extraction)?;

    let mut commands = Vec::with_capacity(found.len());
    for (attr, command) in found {
        edits.push(delete_span(src, attr.span())?);
        commands.push(command);
    }

    let trait_impl = macros_core::command_dispatch_impl(
        &imp.self_ty,
        &imp.generics,
        args.grant.as_ref(),
        &commands,
    )?;
    let pretty = macros_core::pretty_print(&trait_impl);
    edits.push(insert_after_span(
        imp.span(),
        format!("\n\n{}", pretty.trim_end()),
    )?);

    Ok(edits)
}

fn splice(src: &str) -> syn::Result<String> {
    let file = syn::parse_file(src)?;
    let mut edits = Vec::new();

    for item in &file.items {
        match item {
            Item::Use(item_use) if is_macros_use(item_use) => {
                edits.push(delete_span(src, item_use.span())?);
            }
            Item::Impl(imp) if find_attr(&imp.attrs, "syscall").is_some() => {
                edits.extend(syscall_edits(src, imp)?);
            }
            Item::Enum(e) if find_attr(&e.attrs, "subscribe").is_some() => {
                let tokens = macros_core::build_subscribe(e)?;
                edits.push(replace_span(src, e.span(), macros_core::pretty_print(&tokens))?);
            }
            Item::Enum(e) if find_attr(&e.attrs, "allow_read_only").is_some()
                || find_attr(&e.attrs, "allow_read_write").is_some() =>
            {
                edits.push(delete_span(src, e.span())?);
            }
            _ => {}
        }
    }

    edits.sort_by_key(|e| (e.start, e.end));

    let mut out = String::with_capacity(src.len());
    let mut cursor = 0;
    for edit in &edits {
        if edit.start < cursor {
            return Err(syn::Error::new(
                Span::call_site(),
                format!(
                    "overlapping edits at byte {} (already emitted up to {cursor})",
                    edit.start
                ),
            ));
        }
        out.push_str(&src[cursor..edit.start]);
        out.push_str(&edit.replacement);
        cursor = edit.end;
    }
    out.push_str(&src[cursor..]);

    Ok(collapse_blank_lines(&out))
}

/// Deleted items each swallow their own trailing newline but not the blank
/// line that separated them from the next item, so several deletions in a
/// row leave several blank lines behind. Collapsing any run of 3+ `\n`s
/// (i.e. 2+ blank lines) down to one blank line cleans that up without
/// touching spacing anywhere else in the file.
fn collapse_blank_lines(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut newlines = 0;
    for ch in src.chars() {
        if ch == '\n' {
            newlines += 1;
            if newlines > 2 {
                continue;
            }
        } else {
            newlines = 0;
        }
        out.push(ch);
    }
    out
}
