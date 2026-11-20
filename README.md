# syscall-gen-playground

A playground for a Rust proc-macro that generates the boilerplate
`SyscallDriver` implementation for a [Tock OS](https://www.tockos.org/)
capsule from a handful of attributes, instead of writing it by hand. See
[`syscall_drivers/button.rs`](syscall_drivers/button.rs) for the annotated
source and [Why it's shaped this way](#why-its-shaped-this-way) below for
the constraints that drove the design.

## Layout

- [`macros/`](macros/src/lib.rs) — the proc-macro crate. Exports `syscall`,
  `command`, `subscribe`, `allow_read_only`, `allow_read_write`.
- [`macros-core/`](macros-core/src/lib.rs) — the actual parsing/codegen
  logic, as a plain library so it can be reused outside of macro expansion.
  Also builds the `gen-capsule` binary (below).
- [`kernel-mock/`](kernel-mock/src/lib.rs) — a small stand-in for the real
  Tock `kernel` crate (just enough of `syscall`, `grant`, `hil::gpio`,
  `upcall`, `ProcessId`, `ErrorCode`), so capsules can be built and tested
  outside of a real Tock tree.
- [`syscall_drivers/button.rs`](syscall_drivers/button.rs) — an annotated
  capsule, adapted from Tock's real `capsules/core/src/button.rs`.
- [`src/lib.rs`](src/lib.rs) — pulls `button.rs` in as a real module (via
  `#[path]`) against `kernel-mock`, plus a second, simpler `Counter` capsule,
  and tests that exercise both through the macros.

## The attributes

```rust
use macros::{allow_read_only, allow_read_write, command, subscribe, syscall};

#[syscall(grant = apps)]        // `grant` names the capsule's Grant field;
impl<'a, P: ...> Button<'a, P> {// omit it to write `allocate_grant` by hand.
    #[command(0, SuccessU32, Failure)]   // command number, success shape, "Failure" marker
    fn num(&self) -> CommandReturn { ... }
    ...
}

#[subscribe]
enum Subscribe {
    /// doc comment, carried onto the generated UPCALL_* const
    Toggled { pin_num: u32, pressed: bool } = 0,  // = upcall number
}

#[allow_read_only]
enum ROAllow {}   // placeholder for now; validates syntax, generates nothing

#[allow_read_write]
enum RWAllow {}   // same
```

`#[syscall]` generates the `impl SyscallDriver for X { fn command(...)
{...} fn allocate_grant(...) {...} }` block. `#[subscribe]` generates a
`pub mod subscribe { ... }` holding `UPCALL_<NAME>` constants (documented
with the variant's own doc comment plus its inferred `fn(args)` signature),
a `COUNT: u8`, and a typed `schedule_<name>(upcalls, ...)` helper per
variant that packs its named, typed arguments into the `(usize, usize,
usize)` triple `GrantKernelData::schedule_upcall` expects.

## Building and testing

```bash
cargo build
cargo test
```

This builds the whole workspace (`macros`, `macros-core`, `kernel-mock`,
and the `capsule` crate at the root, which pulls in `button.rs`) and runs
the tests in [`src/lib.rs`](src/lib.rs), which drive the generated
`command()`/`allocate_grant`/`schedule_*` code directly.

## Reading what the macros generated

Every macro invocation writes its own expansion, pretty-printed, to
`generated/` (gitignored) next to this `Cargo.toml` — e.g.
`generated/Button_syscall.rs`, `generated/Subscribe_subscribe.rs`. That
folder is populated as a side effect of `cargo build`; if you just deleted
it and nothing else changed, `touch src/lib.rs` first to force a rebuild
(cargo won't re-run a macro it thinks had the same input as last time).

## Producing a complete capsule file

The per-macro dumps above only contain the regenerated item, not the rest
of the file (a proc-macro never sees anything beyond the item it's
attached to, and can't preserve plain `//` comments even in what it does
see). To get a complete, ready-to-drop-into-Tock capsule file — original
comments, doc comments, and unrelated code untouched, byte-for-byte — use
the `gen-capsule` binary instead:

```bash
cargo run -p macros-core --bin gen-capsule -- syscall_drivers/button.rs
```

This writes `generated/button.rs`: the same file, with the `syscall`/
`command`/`subscribe`/`allow_*` attributes resolved away and the generated
code spliced in in their place. It has no dependency on the `macros` crate
and should drop directly into a real Tock capsule file, provided you swap
in the real `kernel` crate for `kernel-mock`. Pass a second argument to
choose a different output path.

## Why it's shaped this way

Rust's attribute macros have no equivalent of a derive macro's `attributes
(...)` registration for inert helper attributes: any attribute written on
an item is independently name-resolved by rustc, even one nested inside an
item that an outer attribute macro goes on to rewrite. And proc-macro
crates can only export macro functions at the crate root — no modules. So:

- `syscall`, `command`, `subscribe`, `allow_read_only`, `allow_read_write`
  all have to be real, top-level `#[proc_macro_attribute]`s, imported
  directly — none of them can live behind a `syscall::` module path, the
  way the attributes were originally sketched.
- `command` never does real work: `#[syscall]` strips every `#[command(..)]`
  it finds before returning, so `command`'s own body only ever runs if it's
  used outside a `#[syscall]` impl, which is a mistake worth a compile error.
- The parsing/codegen logic lives in `macros-core`, a plain library, rather
  than directly in the `macros` proc-macro crate, specifically so
  `gen-capsule` can link against the same logic without going through
  macro expansion at all.
