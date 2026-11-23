fn main() {
    // Resolve from this crate so windres/rc (cwd = OUT_DIR) still finds the .ico.
    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest_dir.join("../../assets/kapturkard-logo.ico");
    let icon = std::fs::canonicalize(&icon).unwrap_or(icon);

    // Re-run when the Windows icon changes.
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-env-changed=CAPTUREVIEW_GIT_HASH");

    // Embed short git hash so Arthur's support report proves which binary he ran.
    let hash = std::env::var("CAPTUREVIEW_GIT_HASH").unwrap_or_else(|_| {
        std::process::Command::new("git")
            .args(["rev-parse", "--short=12", "HEAD"])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    String::from_utf8(o.stdout).ok()
                } else {
                    None
                }
            })
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into())
    });
    println!("cargo:rustc-env=CAPTUREVIEW_GIT_HASH={hash}");
    // Feature stamp: capture-driven FrameReady wake (PR #3 tip).
    println!("cargo:rustc-env=CAPTUREVIEW_CAPTURE_WAKE=1");

    #[cfg(target_os = "windows")]
    {
        if icon.exists() {
            let mut res = winres::WindowsResource::new();
            // Absolute path: relative paths in the generated .rc break when RC runs from OUT_DIR.
            res.set_icon(icon.to_str().expect("icon path utf-8"));
            res.set("ProductName", "KapturKard");
            res.set("FileDescription", "KapturKard");
            if let Err(e) = res.compile() {
                // Fail the build so a missing RC toolchain cannot ship an iconless exe.
                panic!("winres icon embed failed: {e}");
            }
        } else {
            panic!("missing Windows icon at {}", icon.display());
        }
    }
}
