// SPDX-License-Identifier: Apache-2.0
// Gives Toktail.exe Pip's icon and version details on Windows. Elsewhere,
// and if the Windows resource compiler isn't available, it does nothing.
// Everywhere, it checks that VERSION and Cargo.toml agree.
fn main() {
    println!("cargo:rerun-if-changed=packaging/toktail.ico");
    check_version();
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("packaging/toktail.ico")
            .set("ProductName", "Toktail")
            .set("FileDescription", "Toktail: every token leaves a trail")
            .set("LegalCopyright", "Apache License 2.0");
        if let Err(e) = res.compile() {
            println!("cargo:warning=no icon in Toktail.exe: {e}");
        }
    }
}

/// VERSION (kept with `bump`) and Cargo.toml must agree, so a release is
/// never built with a stale number. `make version` copies one to the other.
fn check_version() {
    println!("cargo:rerun-if-changed=VERSION");
    let Ok(text) = std::fs::read_to_string("VERSION") else {
        return;
    };
    let file = text.trim().trim_start_matches('v');
    let cargo = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    if file != cargo {
        panic!("VERSION says {file} but Cargo.toml says {cargo}; run `make version`");
    }
}
