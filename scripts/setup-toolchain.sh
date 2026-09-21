#!/usr/bin/env bash
# Recreate the local Windows toolchain shim used by .cargo/config.toml.
#
# Why this exists: this host has no working MinGW (the rustup-bundled dlltool
# cannot create import libraries and there is no `as`), and the MSVC toolchain
# has no linker (no VC tools / link.exe). So the project builds for the
# LLVM-based x86_64-pc-windows-gnullvm target using a thin rust-lld wrapper
# that picks the correct CRT object per crate type, reusing the GNU target's
# MinGW import libraries. libunwind is shimmed to libgcc_eh.
#
# Usage: scripts/setup-toolchain.sh
set -euo pipefail

GNU=stable-x86_64-pc-windows-gnu
SYSROOT="$(rustc +"$GNU" --print sysroot)"
SC="$(cygpath -w "$SYSROOT")/lib/rustlib/x86_64-pc-windows-gnu/lib/self-contained"
LLD="$(cygpath -w "$SYSROOT")/lib/rustlib/x86_64-pc-windows-gnu/bin/rust-lld.exe"
SHIM="${HOME}/.gnullvm-shim"
mkdir -p "$SHIM"

# libunwind shim (the gnullvm std needs -lunwind; the GNU toolchain provides
# the equivalent in libgcc_eh).
cp "$SYSROOT/lib/rustlib/x86_64-pc-windows-gnu/lib/self-contained/libgcc_eh.a" \
   "$SHIM/libunwind.a"

# Build the rust-lld wrapper.
TMP="$(mktemp -d)"
cat > "$TMP/linker-wrapper.rs" <<RS
use std::path::PathBuf;
use std::process::Command;

const SC: &str = r"$SC";
const LLD: &str = r"$LLD";

fn has_shared(args: &[String]) -> bool {
    for a in args {
        if a == "-shared" {
            return true;
        }
        if let Some(p) = a.strip_prefix('@') {
            if let Ok(c) = std::fs::read_to_string(p) {
                if c.contains("-shared") {
                    return true;
                }
            }
        }
    }
    false
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let crt = if has_shared(&args) { "dllcrt2.o" } else { "crt2.o" };
    let mut forwarded = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-flavor" {
            i += 2;
            continue;
        }
        forwarded.push(args[i].clone());
        i += 1;
    }
    let mut c = Command::new(LLD);
    c.arg("-flavor").arg("gnu");
    c.args(&forwarded);
    c.arg(PathBuf::from(SC).join(crt));
    let st = c.status().expect("spawn rust-lld");
    std::process::exit(st.code().unwrap_or(1));
}
RS
rustc +"$GNU" "$TMP/linker-wrapper.rs" -o "$SHIM/rust-lld.exe"
rm -rf "$TMP"

echo "toolchain shim ready: $SHIM"
echo "  libunwind.a, rust-lld.exe"
echo "verify with: scripts/setup-toolchain.sh && cargo test"
