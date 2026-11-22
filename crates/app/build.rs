fn main() {
    // Re-run when the Windows icon changes.
    let icon = std::path::Path::new("../../assets/kapturkard-logo.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    #[cfg(target_os = "windows")]
    {
        if icon.exists() {
            let mut res = winres::WindowsResource::new();
            res.set_icon(icon.to_str().expect("icon path utf-8"));
            res.set("ProductName", "KapturKard");
            res.set("FileDescription", "KapturKard");
            if let Err(e) = res.compile() {
                println!("cargo:warning=winres icon embed failed: {e}");
            }
        } else {
            println!("cargo:warning=missing {}", icon.display());
        }
    }
}
