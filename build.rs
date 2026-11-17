fn main() {
    println!("cargo:rerun-if-env-changed=APP_VERSION");
    let version = std::env::var("APP_VERSION")
        .ok()
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| format!("{}-dev", env!("CARGO_PKG_VERSION")));
    println!("cargo:rustc-env=APP_VERSION={version}");

    embed_fragments();

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

/// Compile every `assets/fragments/*.json` into the binary: writes `app_fragments.rs` to
/// `OUT_DIR`, a list of (file name, contents) sorted by file name, which `src/fragments.rs`
/// includes. Adding a fragment is adding a file there.
fn embed_fragments() {
    let dir = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("assets")
        .join("fragments");
    // The directory itself, so a file added or removed is noticed, not only one edited.
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .expect("assets/fragments exists")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|x| x == "json") && path.is_file())
        .collect();
    files.sort();
    let mut list = String::from("pub const APP_FRAGMENT_FILES: &[(&str, &str)] = &[\n");
    for path in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path.file_name().unwrap().to_string_lossy();
        list.push_str(&format!(
            "    ({name:?}, include_str!({:?})),\n",
            path.display().to_string()
        ));
    }
    list.push_str("];\n");
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("app_fragments.rs");
    std::fs::write(out, list).expect("OUT_DIR is writable");
}
