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
        // Windows stores each numeric version component in 16 bits. Keep the
        // full version (including a prerelease suffix) in the display strings.
        let core_version = version.split(['-', '+']).next().unwrap();
        let components: Vec<u16> = core_version
            .split('.')
            .map(|component| {
                component
                    .parse()
                    .expect("invalid Windows version component")
            })
            .collect();
        assert_eq!(components.len(), 3, "Windows version must be X.Y.Z");
        let numeric_version = (u64::from(components[0]) << 48)
            | (u64::from(components[1]) << 32)
            | (u64::from(components[2]) << 16);
        res.set("FileVersion", &version)
            .set("ProductVersion", &version)
            .set_version_info(winresource::VersionInfo::FILEVERSION, numeric_version)
            .set_version_info(winresource::VersionInfo::PRODUCTVERSION, numeric_version);
        res.set_icon("resources/icon.ico");
        res.compile().unwrap();
    }

    println!("cargo:rerun-if-changed=resources/icon.ico");
}
