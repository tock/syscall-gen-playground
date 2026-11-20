//! The actual parsing/codegen logic behind the `syscall`/`command`/
//! `subscribe`/`allow_read_only`/`allow_read_write` attributes, factored out
//! of the `macros` proc-macro crate so it can also be driven from a plain
//! binary (`gen-capsule`) that works on whole source *files* rather than one
//! macro-annotated item at a time -- see `bin/gen_capsule.rs`.
//!
//! A proc-macro only ever sees the single item it's attached to, so it can
//! regenerate that item from scratch, but it has no way to hand back "the
//! rest of the file, untouched" -- that's only possible by operating on the
//! file's source text directly, which is what `gen_capsule` does using the
//! span information these parsers attach to each piece they recognize.

use std::collections::BTreeMap;

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::spanned::Spanned;
use syn::{
    Attribute, FnArg, Generics, Ident, ImplItem, ItemEnum, ItemImpl, LitInt, PatType, Token, Type,
};

/// Pretty-prints `tokens` as a standalone Rust file; falls back to the raw
/// (unformatted) token text if they don't parse as a complete set of items.
pub fn pretty_print(tokens: &TokenStream) -> String {
    syn::parse2::<syn::File>(tokens.clone())
        .map(|file| prettyplease::unparse(&file))
        .unwrap_or_else(|_| tokens.to_string())
}

/// The specific `CommandReturn` success constructor a command number is
/// declared to use. A command may still fail with any `ErrorCode`-carrying
/// variant, but every success path it takes must produce this exact
/// variant: TRD104 requires a given command number to always encode its
/// success payload the same way, since userspace decodes it according to a
/// fixed, per-command schema.
#[derive(Clone, Copy)]
pub enum ReturnKind {
    Success,
    SuccessU32,
    SuccessU32U32,
    SuccessU32U32U32,
    SuccessU64,
    SuccessU32U64,
}

impl ReturnKind {
    fn parse_ident(ident: &Ident) -> syn::Result<Self> {
        Ok(match ident.to_string().as_str() {
            "Success" => ReturnKind::Success,
            "SuccessU32" => ReturnKind::SuccessU32,
            "SuccessU32U32" => ReturnKind::SuccessU32U32,
            "SuccessU32U32U32" => ReturnKind::SuccessU32U32U32,
            "SuccessU64" => ReturnKind::SuccessU64,
            "SuccessU32U64" => ReturnKind::SuccessU32U64,
            other => {
                return Err(syn::Error::new(
                    ident.span(),
                    format!(
                        "unknown success return kind `{other}`; expected one of: Success, \
                         SuccessU32, SuccessU32U32, SuccessU32U32U32, SuccessU64, SuccessU32U64"
                    ),
                ))
            }
        })
    }

    /// The `CommandReturn::is_*` predicate that checks for this variant.
    fn is_method_ident(self) -> Ident {
        let name = match self {
            ReturnKind::Success => "is_success",
            ReturnKind::SuccessU32 => "is_success_u32",
            ReturnKind::SuccessU32U32 => "is_success_2_u32",
            ReturnKind::SuccessU32U32U32 => "is_success_3_u32",
            ReturnKind::SuccessU64 => "is_success_u64",
            ReturnKind::SuccessU32U64 => "is_success_u32_u64",
        };
        format_ident!("{name}")
    }

    fn label(self) -> &'static str {
        match self {
            ReturnKind::Success => "success",
            ReturnKind::SuccessU32 => "success_u32",
            ReturnKind::SuccessU32U32 => "success_u32_u32",
            ReturnKind::SuccessU32U32U32 => "success_u32_u32_u32",
            ReturnKind::SuccessU64 => "success_u64",
            ReturnKind::SuccessU32U64 => "success_u32_u64",
        }
    }
}

/// `#[command(<num>, <SuccessVariant>, Failure)]`.
///
/// The trailing `Failure` is required so every call site reads the same
/// way; it doesn't need parsing into anything beyond a sanity check, since
/// every command can fail regardless of its declared success shape (TRD104
/// fixes the *success* encoding per command number, not the failure one).
pub struct CommandSpec {
    pub num: usize,
    pub success: ReturnKind,
}

impl Parse for CommandSpec {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let num_lit: LitInt = input.parse()?;
        let num = num_lit.base10_parse()?;
        input.parse::<Token![,]>()?;
        let success_ident: Ident = input.parse()?;
        let success = ReturnKind::parse_ident(&success_ident)?;
        if !input.is_empty() {
            input.parse::<Token![,]>()?;
            let failure_ident: Ident = input.parse()?;
            if failure_ident != "Failure" {
                return Err(syn::Error::new(
                    failure_ident.span(),
                    "expected `Failure` (every command can fail)",
                ));
            }
        }
        Ok(CommandSpec { num, success })
    }
}

/// `#[syscall]` / `#[syscall(grant = <field>)]`.
///
/// `grant` names the struct field holding the capsule's `Grant` -- the impl
/// block only gives us methods, not the struct definition, so there's no
/// way to find that field on our own. Omit it and `allocate_grant` is left
/// for the capsule to write by hand.
pub struct SyscallArgs {
    pub grant: Option<Ident>,
}

impl Parse for SyscallArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.is_empty() {
            return Ok(SyscallArgs { grant: None });
        }
        let key: Ident = input.parse()?;
        if key != "grant" {
            return Err(syn::Error::new(key.span(), "expected `grant = <field>`"));
        }
        input.parse::<Token![=]>()?;
        let field: Ident = input.parse()?;
        Ok(SyscallArgs { grant: Some(field) })
    }
}

fn is_process_id(ty: &Type) -> bool {
    let Type::Path(path) = ty else { return false };
    path.path
        .segments
        .last()
        .is_some_and(|seg| seg.ident == "ProcessId")
}

fn is_plain_usize(ty: &Type) -> bool {
    let Type::Path(path) = ty else { return false };
    path.path
        .segments
        .last()
        .is_some_and(|seg| seg.ident == "usize")
}

pub struct Command {
    pub num: usize,
    pub method: Ident,
    pub success: ReturnKind,
    pub args: Vec<FnArg>,
}

pub fn ident_of_self_ty(ty: &Type) -> String {
    if let Type::Path(p) = ty {
        if let Some(seg) = p.path.segments.last() {
            return seg.ident.to_string();
        }
    }
    "Driver".to_string()
}

/// Finds every `#[command(..)]`-tagged method in `impl_block`, removes that
/// attribute from it, and returns the attribute (so callers that work on
/// source text can use its original span) paired with the `Command` it
/// described.
pub fn extract_commands(impl_block: &mut ItemImpl) -> syn::Result<Vec<(Attribute, Command)>> {
    let mut by_num: BTreeMap<usize, Ident> = BTreeMap::new();
    let mut found = Vec::new();

    for item in impl_block.items.iter_mut() {
        let ImplItem::Fn(method) = item else { continue };
        let Some(idx) = method
            .attrs
            .iter()
            .position(|a: &Attribute| a.path().is_ident("command"))
        else {
            continue;
        };
        let attr = method.attrs.remove(idx);
        let spec: CommandSpec = attr.parse_args()?;
        if let Some(existing) = by_num.insert(spec.num, method.sig.ident.clone()) {
            return Err(syn::Error::new(
                method.sig.ident.span(),
                format!(
                    "command number {} is already used by `{}`",
                    spec.num, existing
                ),
            ));
        }
        let command = Command {
            num: spec.num,
            method: method.sig.ident.clone(),
            success: spec.success,
            args: method.sig.inputs.iter().skip(1).cloned().collect(),
        };
        found.push((attr, command));
    }

    Ok(found)
}

/// Builds `impl #generics kernel::syscall::SyscallDriver for #self_ty { .. }`
/// (the `command` dispatch plus, if `grant` is given, `allocate_grant`) from
/// an already-extracted command list. This is the part of `#[syscall]`'s
/// output that's genuinely new code; everything else it emits is the
/// original impl block, byte-for-byte.
pub fn command_dispatch_impl(
    self_ty: &Type,
    generics: &Generics,
    grant: Option<&Ident>,
    commands: &[Command],
) -> syn::Result<TokenStream> {
    let by_num: BTreeMap<usize, &Command> = commands.iter().map(|c| (c.num, c)).collect();

    let mut arms = Vec::with_capacity(by_num.len());
    for cmd in by_num.values() {
        let num = Literal::usize_unsuffixed(cmd.num);
        let method = &cmd.method;
        let is_method = cmd.success.is_method_ident();

        let mut data_slots = ["data", "_data2"].into_iter();
        let mut call_args = Vec::with_capacity(cmd.args.len());
        for arg in &cmd.args {
            let FnArg::Typed(PatType { ty, .. }) = arg else {
                return Err(syn::Error::new(
                    arg.span(),
                    "command methods can't take `self` by value",
                ));
            };
            if is_process_id(ty) {
                call_args.push(quote! { processid });
            } else {
                let slot = data_slots.next().ok_or_else(|| {
                    syn::Error::new(
                        ty.span(),
                        "a command can take at most two non-ProcessId arguments \
                         (TRD104's `data` and `data2`)",
                    )
                })?;
                let slot = format_ident!("{slot}");
                call_args.push(if is_plain_usize(ty) {
                    quote! { #slot }
                } else {
                    quote! { (#slot as #ty) }
                });
            }
        }

        let assert_message = Literal::string(&format!(
            "command {} (`{}`) must return CommandReturn::{}(..) on success",
            cmd.num,
            method,
            cmd.success.label(),
        ));

        arms.push(quote! {
            #num => {
                let __command_return: kernel::syscall::CommandReturn =
                    self.#method(#(#call_args),*);
                debug_assert!(
                    __command_return.is_failure()
                        || __command_return.is_failure_u32()
                        || __command_return.is_failure_2_u32()
                        || __command_return.is_failure_u64()
                        || __command_return.#is_method(),
                    #assert_message
                );
                __command_return
            }
        });
    }

    let (impl_generics, _, where_clause) = generics.split_for_impl();

    let allocate_grant = match grant {
        Some(field) => quote! {
            fn allocate_grant(
                &self,
                processid: kernel::ProcessId,
            ) -> Result<(), kernel::process::Error> {
                self.#field.enter(processid, |_, _| {})
            }
        },
        None => quote! {},
    };

    Ok(quote! {
        impl #impl_generics kernel::syscall::SyscallDriver for #self_ty #where_clause {
            fn command(
                &self,
                command_num: usize,
                data: usize,
                _data2: usize,
                processid: kernel::ProcessId,
            ) -> kernel::syscall::CommandReturn {
                match command_num {
                    #(#arms)*
                    // default
                    _ => kernel::syscall::CommandReturn::failure(kernel::ErrorCode::NOSUPPORT),
                }
            }

            #allocate_grant
        }
    })
}

/// `#[syscall]` / `#[syscall(grant = ..)]`'s full output: the (cleaned-up)
/// original impl block followed by the generated `SyscallDriver` impl.
pub fn build_syscall(args: SyscallArgs, mut impl_block: ItemImpl) -> syn::Result<TokenStream> {
    let commands: Vec<Command> = extract_commands(&mut impl_block)?
        .into_iter()
        .map(|(_, c)| c)
        .collect();
    let trait_impl = command_dispatch_impl(
        &impl_block.self_ty,
        &impl_block.generics,
        args.grant.as_ref(),
        &commands,
    )?;
    Ok(quote! {
        #impl_block
        #trait_impl
    })
}

/// `#[subscribe]`'s full output: one `UPCALL_<NAME>: usize` constant per
/// variant. The enum itself (a declarative DSL -- see `syscall_drivers`)
/// is never emitted, only parsed.
/// The doc-comment lines on an item, in order, each still carrying the
/// single leading space `///` leaves (`#[doc = " text"]`) so re-emitting
/// them as `#[doc = #line]` round-trips through `rustfmt`/`prettyplease`
/// looking exactly like a normal `/// text` comment.
fn doc_lines(attrs: &[Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            syn::Meta::NameValue(nv) => match &nv.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) => Some(s.value()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// `Toggled` -> `toggled`, `PinChanged` -> `pin_changed`.
fn pascal_to_snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn named_fields(variant: &syn::Variant) -> syn::Result<Vec<(&Ident, &Type)>> {
    match &variant.fields {
        syn::Fields::Named(fields) => Ok(fields
            .named
            .iter()
            .map(|f| (f.ident.as_ref().expect("named field"), &f.ty))
            .collect()),
        syn::Fields::Unit => Ok(Vec::new()),
        syn::Fields::Unnamed(_) => Err(syn::Error::new(
            variant.span(),
            "subscribe variants must use named fields (or none), not tuple fields, \
             so each upcall argument can be documented by name",
        )),
    }
}

/// `#[subscribe]`'s full output: a `pub mod subscribe` holding one
/// `UPCALL_<NAME>: usize` constant per variant (each documented with the
/// variant's own doc comment plus its upcall signature), a `COUNT` of how
/// many there are, and a `schedule_<name>` helper per variant that packs its
/// named, typed arguments into the `(usize, usize, usize)` triple
/// `GrantKernelData::schedule_upcall` expects -- so callers get type
/// checking on upcall arguments instead of having to get the packing right
/// by hand. The enum itself (a declarative DSL -- see `syscall_drivers`) is
/// never emitted, only parsed.
pub fn build_subscribe(item: &ItemEnum) -> syn::Result<TokenStream> {
    let mut consts = Vec::new();
    let mut fns = Vec::new();
    let mut seen: BTreeMap<usize, Ident> = BTreeMap::new();

    for variant in &item.variants {
        let Some((_, discriminant)) = &variant.discriminant else {
            return Err(syn::Error::new(
                variant.ident.span(),
                "every subscribe variant needs an explicit `= <num>` upcall number",
            ));
        };
        let num: usize = match discriminant {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(lit),
                ..
            }) => lit.base10_parse()?,
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "upcall number must be an integer literal",
                ))
            }
        };
        if let Some(existing) = seen.insert(num, variant.ident.clone()) {
            return Err(syn::Error::new(
                variant.ident.span(),
                format!("upcall number {num} is already used by `{existing}`"),
            ));
        }

        let fields = named_fields(variant)?;
        if fields.len() > 3 {
            return Err(syn::Error::new(
                variant.span(),
                "an upcall can carry at most 3 arguments -- TRD104 only gives it \
                 three usize-sized words to pack them into",
            ));
        }

        let const_name = format_ident!("UPCALL_{}", variant.ident.to_string().to_uppercase());
        let num_lit = Literal::usize_unsuffixed(num);

        let signature = format!(
            "fn({})",
            fields
                .iter()
                .map(|(name, ty)| format!("{name}: {}", quote!(#ty)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let mut doc = doc_lines(&variant.attrs);
        if !doc.is_empty() {
            doc.push(String::new());
        }
        doc.push(format!(" Upcall signature: `{signature}`"));
        let doc_attrs = doc.iter().map(|line| quote! { #[doc = #line] });

        consts.push(quote! {
            #(#doc_attrs)*
            pub const #const_name: usize = #num_lit;
        });

        let fn_name = format_ident!("schedule_{}", pascal_to_snake(&variant.ident.to_string()));
        let params = fields.iter().map(|(name, ty)| quote! { #name: #ty });
        let mut tuple_elems: Vec<TokenStream> = fields
            .iter()
            .map(|(name, ty)| {
                if is_plain_usize(ty) {
                    quote! { #name }
                } else {
                    quote! { (#name as usize) }
                }
            })
            .collect();
        tuple_elems.resize_with(3, || quote! { 0 });
        let fn_doc = format!(" Schedules the `{}` upcall.", variant.ident);

        fns.push(quote! {
            #[doc = #fn_doc]
            pub fn #fn_name(
                upcalls: &kernel::grant::GrantKernelData<'_>,
                #(#params),*
            ) -> Result<(), kernel::upcall::UpcallError> {
                upcalls.schedule_upcall(#const_name, (#(#tuple_elems),*))
            }
        });
    }

    let count = Literal::u8_suffixed(seen.len() as u8);
    consts.push(quote! {
        #[doc = " The number of upcalls."]
        pub const COUNT: u8 = #count;
    });

    Ok(quote! {
        pub mod subscribe {
            #(#consts)*
            #(#fns)*
        }
    })
}
