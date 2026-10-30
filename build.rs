fn main() {
    println!("cargo:rerun-if-env-changed=APP_VERSION");
    let version = std::env::var("APP_VERSION")
        .ok()
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| format!("{}-dev", env!("CARGO_PKG_VERSION")));
    println!("cargo:rustc-env=APP_VERSION={version}");

    // Embed the icon into the .exe so Explorer and the taskbar show it even
    // before the app creates a window.
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("resources/icon.ico");
        res.compile().unwrap();
    }

    println!("cargo:rerun-if-changed=resources/icon.ico");
}
