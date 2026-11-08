fn main() {
    let sha = std::env::var("GLYPHCAM_BUILD_SHA")
        .ok()
        .or_else(|| {
            std::process::Command::new("git")
                .args(["rev-parse", "--short=8", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        })
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=GLYPHCAM_BUILD_SHA={sha}");
    let channel = std::env::var("GLYPHCAM_CHANNEL").unwrap_or_else(|_| "dev".into());
    println!("cargo:rustc-env=GLYPHCAM_CHANNEL={channel}");
    println!("cargo:rerun-if-env-changed=GLYPHCAM_BUILD_SHA");
    println!("cargo:rerun-if-env-changed=GLYPHCAM_CHANNEL");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=../../packaging/windows/glyphcam.rc");
        let _ =
            embed_resource::compile("../../packaging/windows/glyphcam.rc", embed_resource::NONE);
    }
}
