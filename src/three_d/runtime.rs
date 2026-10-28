//! Where a released app keeps OpenBabel's format plugins and data, so OpenBabel finds them.
//!
//! openbabel_rs points OpenBabel at its plugin and data directories once, in
//! [`openbabel::init`]: at the application-relative layout when there is a `data` directory
//! beside the executable (`<exe_dir>` for the plugins, `<exe_dir>/data`), otherwise at the paths
//! of the build machine's OpenBabel. The Windows release uses that layout as it is. The Unix
//! packages cannot — a `data` directory in `/usr/bin` or in a macOS bundle's `Contents/MacOS`
//! does not belong there — so they install OpenBabel where their platform keeps such files,
//! and [`init`] points OpenBabel there instead:
//!
//! | Package          | Executable                    | Plugins (`BABEL_LIBDIR`)               | Data (`BABEL_DATADIR`)                   |
//! | ---------------- | ----------------------------- | -------------------------------------- | ---------------------------------------- |
//! | .deb, AppImage   | `<prefix>/bin/`               | `<prefix>/lib/open-draw-chemistry/openbabel/` | `<prefix>/share/open-draw-chemistry/openbabel/` |
//! | macOS .app       | `Contents/MacOS/`             | `Contents/Frameworks/openbabel/`        | `Contents/Resources/openbabel/`          |
//!
//! The shared libraries themselves (`libopenbabel`, `libinchi`) are found by the dynamic loader
//! through the executable's rpath, which the release workflow sets to the directory above the
//! plugins' (`$ORIGIN/../lib/open-draw-chemistry`, `@executable_path/../Frameworks`); the
//! plugins' own rpath (`$ORIGIN/..`, `@loader_path/..`) finds them from there.

use std::path::{Path, PathBuf};

/// The plugin and data directories of a packaged Unix app whose executable is in `exe_dir`,
/// when both are there.
pub fn bundled_dirs(exe_dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let up = exe_dir.parent()?;
    [
        // Linux .deb and AppImage: <prefix>/bin/open_draw_chemistry.
        (
            up.join("lib/open-draw-chemistry/openbabel"),
            up.join("share/open-draw-chemistry/openbabel"),
        ),
        // macOS: Open DrawChemistry.app/Contents/MacOS/open_draw_chemistry.
        (
            up.join("Frameworks/openbabel"),
            up.join("Resources/openbabel"),
        ),
    ]
    .into_iter()
    .find(|(plugins, data)| plugins.is_dir() && data.is_dir())
}

/// Point OpenBabel at the plugins and data the app was installed with. Call it at the very start
/// of `main`, before any other thread exists and before anything uses OpenBabel.
pub fn init() {
    openbabel::init();
    #[cfg(unix)]
    {
        let exe = std::env::current_exe().ok();
        if let Some((plugins, data)) = exe.as_deref().and_then(Path::parent).and_then(bundled_dirs)
        {
            // `openbabel::init` has set both variables, once and for good; overriding them now
            // is what OpenBabel reads when it first loads its plugins and data (lazily, on
            // first use). On Unix `std::env::set_var` is the C library's `setenv`, which
            // OpenBabel's `getenv` sees.
            //
            // SAFETY: called at the start of `main`, while the process is still single-threaded,
            // so nothing reads the environment concurrently.
            unsafe {
                std::env::set_var("BABEL_LIBDIR", plugins);
                std::env::set_var("BABEL_DATADIR", data);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("odc-obruntime-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn the_linux_layout_is_found_beside_bin() {
        let root = temp("linux");
        std::fs::create_dir_all(root.join("usr/bin")).unwrap();
        assert_eq!(
            bundled_dirs(&root.join("usr/bin")),
            None,
            "not installed yet"
        );
        let plugins = root.join("usr/lib/open-draw-chemistry/openbabel");
        let data = root.join("usr/share/open-draw-chemistry/openbabel");
        std::fs::create_dir_all(&plugins).unwrap();
        assert_eq!(bundled_dirs(&root.join("usr/bin")), None, "no data");
        std::fs::create_dir_all(&data).unwrap();
        assert_eq!(bundled_dirs(&root.join("usr/bin")), Some((plugins, data)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_macos_layout_is_found_in_the_bundle() {
        let root = temp("macos").join("Open DrawChemistry.app/Contents");
        let plugins = root.join("Frameworks/openbabel");
        let data = root.join("Resources/openbabel");
        std::fs::create_dir_all(root.join("MacOS")).unwrap();
        std::fs::create_dir_all(&plugins).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        assert_eq!(bundled_dirs(&root.join("MacOS")), Some((plugins, data)));
        let _ = std::fs::remove_dir_all(temp("macos"));
    }

    /// A cargo target directory (tests, `cargo run`) keeps the build machine's OpenBabel.
    #[test]
    fn a_build_tree_has_no_bundle() {
        let exe = std::env::current_exe().unwrap();
        assert_eq!(bundled_dirs(exe.parent().unwrap()), None);
    }
}
