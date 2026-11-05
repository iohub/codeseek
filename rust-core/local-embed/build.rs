//! macOS 链接修复：为静态链接的 ONNX Runtime 补上 `___isPlatformVersionAtLeast`。
//!
//! P0 实测结论（见 docs/local-embedding-backend.md 的实施记录）：
//! ort-sys 的 `download-binaries` 静态库里，CoreML EP 的 `host_utils.mm.o` / `model.mm.o`
//! 引用了 clang 的 availability helper `___isPlatformVersionAtLeast`。该符号：
//! - **不在** macOS SDK 的任何 `.tbd` 里（`libSystem` 不导出）；
//! - **不在** Rust sysroot 的 `libcompiler_builtins-*.rlib` 里；
//! - 只在 clang 自带的 `lib<rd>/lib/darwin/libclang_rt.osx.a` 里有定义。
//!
//! clang 直接编译链 C 程序时会自动带上该归档，rustc 不会 —— 它用 `-nodefaultlibs`，
//! 只链 sysroot 的 compiler_builtins。于是 `ld: symbol(s) not found`。
//!
//! 与 LTO 无关：`lto = "fat"` 与 `"thin"` 都失败，加上本归档后 `lto = "fat"` 通过。
//! 也与 `MACOSX_DEPLOYMENT_TARGET` 无关：设为 13.4 后（链接行里确实带了
//! `-mmacosx-version-min=13.4`）仍然失败。
//!
//! 这里用 Apple 自己的实现（而非手写桩），避免availability 判断的语义偏差。

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CC");
    println!("cargo:rerun-if-env-changed=CC_x86_64_apple_darwin");

    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.ends_with("apple-darwin") {
        // Linux/Windows 上 ONNX Runtime 不走这条符号引用路径。
        return;
    }

    let Some(resource_dir) = clang_resource_dir() else {
        // 没有 clang 工具链（如纯 CI 容器）时不要静默失败：让链接错误照常发生，
        // 由日志里这一行提示定位原因。
        println!("cargo:warning=clang -print-resource-dir failed; CoreML EP symbols may not resolve");
        return;
    };

    let darwin_dir = PathBuf::from(resource_dir).join("lib").join("darwin");
    if darwin_dir.join("libclang_rt.osx.a").exists() {
        println!("cargo:rustc-link-search=native={}", darwin_dir.display());
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
    }
}

/// `clang -print-resource-dir`，输出形如
/// `/Library/Developer/CommandLineTools/usr/lib/clang/21`。
///
/// 走 `xcrun` 会漏掉非 Xcode 工具链，故直接问 `$CC`（缺省 `clang`）。
fn clang_resource_dir() -> Option<String> {
    let cc = std::env::var("CC").unwrap_or_else(|_| "clang".to_string());
    let out = Command::new(&cc).arg("-print-resource-dir").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let dir = String::from_utf8(out.stdout).ok()?;
    let dir = dir.trim().to_string();
    (!dir.is_empty()).then_some(dir)
}
