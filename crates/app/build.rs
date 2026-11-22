fn main() {
    // Re-run when the Windows icon changes.
    let icon = std::path::Path::new("../../assets/kapturkard-logo.ico");
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
