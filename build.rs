// SPDX-License-Identifier: Apache-2.0
// Gives Toktail.exe Pip's icon and version details on Windows. Elsewhere,
// and if the Windows resource compiler isn't available, it does nothing.
fn main() {
    println!("cargo:rerun-if-changed=packaging/toktail.ico");
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
