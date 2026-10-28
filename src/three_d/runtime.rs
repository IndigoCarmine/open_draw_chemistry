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

/// The plugin and data directories OpenBabel uses in an app installed with its executable in
/// `exe_dir`: Windows' `<exe_dir>` and `<exe_dir>/data` (what [`openbabel::init`] picks), or the
/// Unix packages' [`bundled_dirs`]. `None` in a build tree, which uses the build machine's.
pub fn installed_dirs(exe_dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let data = exe_dir.join("data");
    if data.is_dir() {
        return Some((exe_dir.to_path_buf(), data));
    }
    bundled_dirs(exe_dir)
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

/// Exercise the installed OpenBabel the way the app uses it — read a 3D file (a format
/// plugin), generate and relax a model under each force field (the force-field plugin and its
/// data), write a SMILES and export the model — and describe what came out. For
/// `open_draw_chemistry --check-openbabel`, which the release workflow runs on each package to
/// prove that the shipped app finds its OpenBabel. Call [`init`] first.
///
/// With `installed`, the executable must also be in an installed layout ([`installed_dirs`])
/// whose plugins and data are there: OpenBabel compiles much of its data in, so a model comes
/// out even when the data directory is missing — only worse (no ring templates).
pub fn self_check(installed: bool) -> Result<String, String> {
    use chembuider_rs::{BondOrder, Config, Molecule};

    use super::{Export3d, ForceField, read};

    let mut report = format!("OpenBabel {}\n", openbabel::version());
    fn fail(report: &str, what: &str, e: impl std::fmt::Display) -> String {
        format!("{report}{what}: {e}")
    }

    if installed {
        let exe = std::env::current_exe().map_err(|e| fail(&report, "executable", e))?;
        let exe_dir = exe.parent().unwrap_or(Path::new("."));
        let Some((plugins, data)) = installed_dirs(exe_dir) else {
            return Err(fail(
                &report,
                "installation",
                format!("no OpenBabel plugins and data beside {}", exe.display()),
            ));
        };
        // The force-field plugin (`plugin_forcefields.obf` / `.so`) and the 3D builder's ring
        // templates stand in for "all of them".
        let has_force_fields = std::fs::read_dir(&plugins)
            .map_err(|e| fail(&report, "plugins", e))?
            .flatten()
            .any(|f| {
                f.file_name()
                    .to_string_lossy()
                    .starts_with("plugin_forcefields.")
            });
        if !has_force_fields {
            let why = format!("no force fields in {}", plugins.display());
            return Err(fail(&report, "plugins", why));
        }
        if !data.join("ring-fragments.txt").is_file() {
            let why = format!("no ring templates in {}", data.display());
            return Err(fail(&report, "data", why));
        }
        report += &format!("plugins: {}\ndata: {}\n", plugins.display(), data.display());
    }

    let xyz = "3\nwater\nO 0.0 0.0 0.1173\nH 0.0 0.7572 -0.4692\nH 0.0 -0.7572 -0.4692\n";
    let opened = read::read(read::Format3d::Xyz, xyz).map_err(|e| fail(&report, "XYZ read", e))?;
    let atoms = opened.model.map_or(0, |m| m.atoms.len());
    if atoms != 3 {
        return Err(fail(
            &report,
            "XYZ read",
            format!("{atoms} atoms instead of 3"),
        ));
    }
    report += "XYZ read: 3 atoms\n";

    let mut ethanol = Molecule::default();
    let c1 = ethanol.add_atom("C".into(), [0.0, 0.0], 0);
    let c2 = ethanol.add_atom("C".into(), [1.3, 0.75], 0);
    let o = ethanol.add_atom("O".into(), [2.6, 0.0], 0);
    ethanol.add_bond(c1, c2, BondOrder::Single);
    ethanol.add_bond(c2, o, BondOrder::Single);
    let config = Config::default();
    for force_field in ForceField::ALL {
        let model = super::generate(&ethanol, &config, force_field)
            .map_err(|e| fail(&report, "3D generation", e))?;
        if model.atoms.len() != 9 || !model.energy.is_finite() {
            return Err(fail(
                &report,
                "3D generation",
                format!("{} atoms, E = {}", model.atoms.len(), model.energy),
            ));
        }
        report += &format!(
            "3D {force_field}: {} ({} atoms), E = {:.3} {}\n",
            model.formula,
            model.atoms.len(),
            model.energy,
            model.energy_unit
        );
        if force_field == ForceField::default() {
            for format in Export3d::ALL {
                super::export(&model, format).map_err(|e| fail(&report, "export", e))?;
            }
            report += "export: mol, sdf, xyz, pdb, mol2\n";
        }
    }

    let smiles =
        crate::smiles::compute(&ethanol, &config).map_err(|e| fail(&report, "SMILES", e))?;
    if smiles != "CCO" {
        return Err(fail(&report, "SMILES", format!("{smiles} instead of CCO")));
    }
    report += "SMILES: CCO\n";
    Ok(report)
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

    #[test]
    fn the_self_check_passes_against_the_build_machines_openbabel() {
        init();
        let report = self_check(false).unwrap_or_else(|e| panic!("{e}"));
        assert!(report.contains("SMILES: CCO"), "{report}");
    }

    /// A cargo target directory (tests, `cargo run`) keeps the build machine's OpenBabel.
    #[test]
    fn a_build_tree_has_no_bundle() {
        let exe = std::env::current_exe().unwrap();
        assert_eq!(bundled_dirs(exe.parent().unwrap()), None);
    }
}
