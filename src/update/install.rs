//! How this copy was installed, and installing a verified download over it.
//!
//! [`prepare`] does everything that can fail while the app is still open, downloading and
//! verifying included, and [`Prepared::apply`] does the rest as the app exits.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::time::Duration;

use tempfile::{TempDir, TempPath};

use super::http::{self, Transport};
use super::release::Download;
use super::{Error, signature};

/// Starts every directory and file an update leaves in the temporary directory or beside the
/// app.
const TEMP_PREFIX: &str = ".open-draw-chemistry-update-";

/// How old a leftover download must be to be deleted, so that one in use by another running
/// copy of the app is left alone.
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// A signature file is 64 bytes.
const SIGNATURE_LIMIT: u64 = 1024;

/// The uninstall key Inno Setup writes for `AppId` in installer/windows.iss, in the
/// current user's hive for a per-user install and the machine's for an all-users one.
#[cfg(windows)]
const INNO_UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\\
                                  {6F4D1816-8E52-4698-955B-A19E832D150F}_is1";

/// Who an Inno Setup install belongs to, which decides what the update must ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Per user: needs no administrator rights.
    CurrentUser,
    /// Machine-wide: the installer asks for elevation (UAC).
    AllUsers,
}

/// How this copy of the app was installed, as far as updating it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallFormat {
    /// The Inno Setup installer; `exe` is the installed executable.
    WindowsInstaller { scope: Scope, exe: PathBuf },
    /// This AppImage file.
    AppImage(PathBuf),
    /// This `.app` bundle.
    MacApp(PathBuf),
    /// The Windows portable zip, a .deb, a development build, or anything else.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Linux,
    MacOs,
    Other,
}

/// Where Inno Setup says the app is installed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InnoRecords {
    pub current_user: Option<PathBuf>,
    pub all_users: Option<PathBuf>,
}

/// What [`detect`] looks at.
#[derive(Debug, Clone)]
pub struct Environment {
    pub os: Os,
    pub exe: PathBuf,
    /// `APPIMAGE` and `APPDIR`, which an AppImage's runtime sets.
    pub appimage: Option<PathBuf>,
    pub appdir: Option<PathBuf>,
    pub inno: InnoRecords,
}

impl Environment {
    pub fn current() -> Self {
        let os = if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Other
        };
        Self {
            os,
            exe: std::env::current_exe().unwrap_or_default(),
            appimage: std::env::var_os("APPIMAGE")
                .map(PathBuf::from)
                .filter(|path| path.is_file()),
            appdir: std::env::var_os("APPDIR").map(PathBuf::from),
            inno: inno_records(),
        }
    }
}

#[cfg(windows)]
fn inno_records() -> InnoRecords {
    use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    use crate::ole_server::register::read_string;
    let location =
        |root: HKEY| read_string(root, INNO_UNINSTALL_KEY, "InstallLocation").map(PathBuf::from);
    InnoRecords {
        current_user: location(HKEY_CURRENT_USER),
        all_users: location(HKEY_LOCAL_MACHINE),
    }
}

#[cfg(not(windows))]
fn inno_records() -> InnoRecords {
    InnoRecords::default()
}

/// How the copy described by `environment` was installed. Only what is known for certain counts:
/// anything in doubt is [`InstallFormat::Other`], which is sent to the Release page.
pub fn detect(environment: &Environment) -> InstallFormat {
    let exe = &environment.exe;
    match environment.os {
        Os::Windows => {
            // The portable zip runs from a folder Inno Setup has no record of.
            let inno = &environment.inno;
            let records = [
                (&inno.current_user, Scope::CurrentUser),
                (&inno.all_users, Scope::AllUsers),
            ];
            records
                .into_iter()
                .find(|(record, _)| record.as_ref().is_some_and(|dir| runs_from(exe, dir)))
                .map_or(InstallFormat::Other, |(_, scope)| {
                    InstallFormat::WindowsInstaller {
                        scope,
                        exe: exe.clone(),
                    }
                })
        }
        Os::Linux => match (&environment.appimage, &environment.appdir) {
            // A child process inherits these, so the executable must be inside the mount.
            (Some(appimage), Some(appdir)) if exe.starts_with(appdir) => {
                InstallFormat::AppImage(appimage.clone())
            }
            _ => InstallFormat::Other,
        },
        Os::MacOs => bundle_of(exe).map_or(InstallFormat::Other, InstallFormat::MacApp),
        Os::Other => InstallFormat::Other,
    }
}

/// Whether the executable `exe` sits directly in `dir`, compared as Windows compares paths
/// (and on any OS, so that this can be tested on any).
fn runs_from(exe: &Path, dir: &Path) -> bool {
    let normal = |path: &Path| {
        path.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    normal(exe)
        .strip_prefix(&normal(dir))
        .and_then(|rest| rest.strip_prefix('\\'))
        .is_some_and(|name| !name.is_empty() && !name.contains('\\'))
}

/// The `.app` bundle holding `exe` when it can be replaced: not one still on the disk image, and
/// not one Gatekeeper is running from a randomised read-only copy (App Translocation).
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent().filter(|dir| dir.ends_with("MacOS"))?;
    let contents = macos.parent().filter(|dir| dir.ends_with("Contents"))?;
    let bundle = contents.parent()?;
    let is_app = bundle
        .extension()
        .is_some_and(|extension| extension == "app");
    let read_only = bundle.starts_with("/Volumes")
        || bundle
            .components()
            .any(|component| component.as_os_str() == "AppTranslocation");
    (is_app && !read_only).then(|| bundle.to_owned())
}

/// A verified update, ready to install.
#[derive(Debug)]
pub enum Prepared {
    /// `temp` holds `setup`.
    WindowsInstaller {
        setup: PathBuf,
        scope: Scope,
        exe: PathBuf,
        temp: TempDir,
    },
    /// `staged` is the new AppImage, beside `target`, so that a rename replaces it.
    AppImage { staged: TempPath, target: PathBuf },
    /// `staging` holds the new bundle, beside `target` for the same reason.
    MacApp { staging: TempDir, target: PathBuf },
}

/// Download `download` into a temporary directory, verify it and get it ready to install. The
/// directory is deleted again unless the update is applied. A failed verification ends here,
/// before anything has touched the install.
pub fn prepare(
    transport: &dyn Transport,
    download: &Download,
    cancel: &AtomicBool,
    progress: &AtomicU64,
) -> Result<Prepared, Error> {
    let temp = tempfile::Builder::new().prefix(TEMP_PREFIX).tempdir()?;
    let signature = http::read_limited(
        transport,
        &download.signature.browser_download_url,
        SIGNATURE_LIMIT,
    )?;
    let path = temp.path().join(&download.asset.name);
    let mut file = File::create(&path)?;
    http::download(
        transport,
        &download.asset.browser_download_url,
        download.asset.size,
        &mut file,
        cancel,
        progress,
    )?;
    file.sync_all()?;
    drop(file);
    signature::verify_file(&download.key, &download.asset.name, &path, &signature)?;

    match &download.format {
        InstallFormat::WindowsInstaller { scope, exe } => Ok(Prepared::WindowsInstaller {
            setup: path,
            scope: *scope,
            exe: exe.clone(),
            temp,
        }),
        InstallFormat::AppImage(target) => Ok(Prepared::AppImage {
            staged: stage_file(&path, target)?,
            target: target.clone(),
        }),
        InstallFormat::MacApp(target) => {
            let staging = stage_bundle_dir(target)?;
            copy_app_from_dmg(&path, &new_bundle(staging.path()))?;
            Ok(Prepared::MacApp {
                staging,
                target: target.clone(),
            })
        }
        InstallFormat::Other => Err(Error::Install("unsupported format".to_owned())),
    }
}

/// A complete copy of `source` beside `target`, with `target`'s permissions (so an AppImage
/// stays executable), ready to rename over it. It is deleted again unless it is persisted.
fn stage_file(source: &Path, target: &Path) -> Result<TempPath, Error> {
    let dir = target
        .parent()
        .ok_or_else(|| Error::Install(target.display().to_string()))?;
    let mut staged = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .tempfile_in(dir)?;
    std::io::copy(&mut File::open(source)?, staged.as_file_mut())?;
    staged
        .as_file()
        .set_permissions(std::fs::metadata(target)?.permissions())?;
    staged.as_file().sync_all()?;
    Ok(staged.into_temp_path())
}

/// A directory beside the bundle `target` to build the new bundle in.
fn stage_bundle_dir(target: &Path) -> Result<TempDir, Error> {
    let dir = target
        .parent()
        .ok_or_else(|| Error::Install(target.display().to_string()))?;
    Ok(tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .tempdir_in(dir)?)
}

fn new_bundle(staging: &Path) -> PathBuf {
    staging.join("new.app")
}

fn old_bundle(staging: &Path) -> PathBuf {
    staging.join("old.app")
}

/// Run `command`, which must succeed.
fn run(command: &mut Command) -> Result<(), Error> {
    let status = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Install(format!("{command:?}: {status}")))
    }
}

/// Copy the app out of the disk image `dmg` to `to`.
fn copy_app_from_dmg(dmg: &Path, to: &Path) -> Result<(), Error> {
    let mount = tempfile::Builder::new().prefix(TEMP_PREFIX).tempdir()?;
    run(Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-noautoopen", "-readonly"])
        .arg("-mountpoint")
        .arg(mount.path())
        .arg(dmg))?;
    let bundle = mount.path().join(format!("{}.app", crate::APP_NAME));
    let copied = run(Command::new("ditto").arg(&bundle).arg(to));
    let detached = run(Command::new("hdiutil")
        .args(["detach", "-force"])
        .arg(mount.path()));
    copied.and(detached)
}

/// Put the bundle `new` in `target`'s place, keeping the old one at `old` for as long as that
/// might fail.
fn swap_bundle(new: &Path, target: &Path, old: &Path) -> Result<(), Error> {
    std::fs::rename(target, old)?;
    if let Err(e) = std::fs::rename(new, target) {
        std::fs::rename(old, target)?;
        return Err(e.into());
    }
    Ok(())
}

/// The Inno Setup command line of a silent update (see the "Setup Command Line Parameters" of
/// its documentation): `/CLOSEAPPLICATIONS` for other running copies of the app, and the mode
/// the app was installed in, so that the update lands on that install and asks for elevation
/// only when it was machine-wide.
fn setup_arguments(scope: Scope) -> String {
    let mode = match scope {
        Scope::CurrentUser => "/CURRENTUSER",
        Scope::AllUsers => "/ALLUSERS",
    };
    format!("/SILENT /SUPPRESSMSGBOXES /NORESTART /CLOSEAPPLICATIONS {mode}")
}

/// What cmd.exe runs once the app has gone, since the installer replaces the running executable
/// and the app can no longer do this itself: wait for the installer, go to the Release page if
/// it failed (or UAC was declined), start the app, and delete the download. The paths are in
/// environment variables so that nothing in them needs quoting for cmd.exe, and the commands
/// around them are always the same.
const SETUP_SCRIPT: &str = concat!(
    r#"start "" /wait "%ODC_SETUP%" %ODC_SETUP_ARGS%"#,
    r#" & (if errorlevel 1 start "" "%ODC_RELEASE_PAGE%")"#,
    r#" & start "" "%ODC_APP%""#,
    r#" & rmdir /s /q "%ODC_UPDATE_DIR%""#,
);

/// The command that runs [`SETUP_SCRIPT`].
fn setup_command(
    setup: &Path,
    scope: Scope,
    exe: &Path,
    temp: &Path,
    release_page: &str,
) -> Command {
    // Quoting is cmd.exe's own: /S drops the outermost pair and takes the rest literally.
    let arguments = format!("/D /S /C \"{SETUP_SCRIPT}\"");
    let mut command = Command::new("cmd.exe");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.raw_arg(arguments).creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    command.arg(arguments);
    command
        .env("ODC_SETUP", setup)
        .env("ODC_SETUP_ARGS", setup_arguments(scope))
        .env("ODC_RELEASE_PAGE", release_page)
        .env("ODC_APP", exe)
        .env("ODC_UPDATE_DIR", temp)
        // Not in the directory it is going to delete.
        .current_dir(temp.parent().unwrap_or(temp));
    command
}

/// Start `command`, which takes it from here, and leave it running.
fn relaunch(command: &mut Command) -> Result<(), Error> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

impl Prepared {
    /// Install the update and start the new version, as the app exits. `release_page` is where
    /// a Windows installer that fails later sends the user; an error here is for the caller to
    /// send them there.
    pub fn apply(self, release_page: &str) -> Result<(), Error> {
        match self {
            Prepared::WindowsInstaller {
                setup,
                scope,
                exe,
                temp,
            } => {
                relaunch(&mut setup_command(
                    &setup,
                    scope,
                    &exe,
                    temp.path(),
                    release_page,
                ))?;
                // The installer is running from it; the script above deletes it afterwards.
                let _ = temp.keep();
                Ok(())
            }
            Prepared::AppImage { staged, target } => {
                staged.persist(&target).map_err(|e| e.error)?;
                relaunch(&mut Command::new(&target))
            }
            Prepared::MacApp { staging, target } => {
                let staging_path = staging.path();
                swap_bundle(
                    &new_bundle(staging_path),
                    &target,
                    &old_bundle(staging_path),
                )?;
                relaunch(Command::new("open").arg("-n").arg(&target))
            }
        }
    }
}

/// Delete what an update left in `dir` when the app was killed mid-download.
pub fn remove_stale(dir: &Path) {
    remove_older_than(dir, STALE_AFTER);
}

fn remove_older_than(dir: &Path, age: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry.metadata().is_ok_and(|metadata| {
            metadata.is_dir()
                && metadata
                    .modified()
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|elapsed| elapsed >= age)
        });
        if stale && entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX) {
            // Another user's directory, or one in use, is simply left there.
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};

    use super::super::http::testing::{Canned, FakeTransport};
    use super::super::release::Asset;
    use super::super::signature::{PublicKey, signed_message};
    use super::*;

    fn environment(os: Os, exe: &str) -> Environment {
        Environment {
            os,
            exe: PathBuf::from(exe),
            appimage: None,
            appdir: None,
            inno: InnoRecords::default(),
        }
    }

    #[test]
    fn a_per_user_windows_install_is_detected() {
        let exe = r"C:\Users\me\AppData\Local\Programs\Open DrawChemistry\app.exe";
        let mut env = environment(Os::Windows, exe);
        env.inno.current_user =
            Some(r"C:\Users\me\AppData\Local\Programs\Open DrawChemistry\".into());
        assert_eq!(
            detect(&env),
            InstallFormat::WindowsInstaller {
                scope: Scope::CurrentUser,
                exe: exe.into()
            }
        );
    }

    #[test]
    fn an_all_users_windows_install_is_detected_whatever_the_case() {
        let exe = r"c:\program files\Open DrawChemistry\app.exe";
        let mut env = environment(Os::Windows, exe);
        env.inno.all_users = Some(r"C:\Program Files\Open DrawChemistry\".into());
        assert_eq!(
            detect(&env),
            InstallFormat::WindowsInstaller {
                scope: Scope::AllUsers,
                exe: exe.into()
            }
        );
    }

    /// Each record is only believed for its own directory: with both installs on the machine
    /// the one running is the one updated.
    #[test]
    fn the_record_of_the_running_copy_decides_the_scope() {
        let mut env = environment(Os::Windows, r"C:\Program Files\ODC\app.exe");
        env.inno.current_user = Some(r"C:\Users\me\ODC\".into());
        env.inno.all_users = Some(r"C:\Program Files\ODC\".into());
        assert!(matches!(
            detect(&env),
            InstallFormat::WindowsInstaller {
                scope: Scope::AllUsers,
                ..
            }
        ));
    }

    /// The portable zip has no record for its folder, even on a machine that also has the
    /// installed app.
    #[test]
    fn a_portable_windows_copy_is_not_an_install() {
        let mut env = environment(Os::Windows, r"D:\tools\odc\app.exe");
        assert_eq!(detect(&env), InstallFormat::Other);
        env.inno.all_users = Some(r"C:\Program Files\ODC\".into());
        assert_eq!(detect(&env), InstallFormat::Other);
        // Nor does a folder that merely starts the same way, or a subfolder, count.
        env.exe = r"C:\Program Files\ODC2\app.exe".into();
        assert_eq!(detect(&env), InstallFormat::Other);
        env.exe = r"C:\Program Files\ODC\sub\app.exe".into();
        assert_eq!(detect(&env), InstallFormat::Other);
    }

    #[test]
    fn an_appimage_is_detected() {
        let mut env = environment(Os::Linux, "/tmp/.mount_abc/usr/bin/app");
        env.appimage = Some("/home/me/App.AppImage".into());
        env.appdir = Some("/tmp/.mount_abc".into());
        assert_eq!(
            detect(&env),
            InstallFormat::AppImage("/home/me/App.AppImage".into())
        );
    }

    /// A .deb app started from an AppImage's terminal inherits its `APPIMAGE`.
    #[test]
    fn an_inherited_appimage_variable_is_not_this_copy() {
        let mut env = environment(Os::Linux, "/usr/bin/app");
        assert_eq!(detect(&env), InstallFormat::Other);
        env.appimage = Some("/home/me/Other.AppImage".into());
        env.appdir = Some("/tmp/.mount_abc".into());
        assert_eq!(detect(&env), InstallFormat::Other);
        env.appdir = None;
        assert_eq!(detect(&env), InstallFormat::Other);
    }

    #[test]
    fn a_mac_bundle_is_detected() {
        let env = environment(
            Os::MacOs,
            "/Applications/Open DrawChemistry.app/Contents/MacOS/app",
        );
        assert_eq!(
            detect(&env),
            InstallFormat::MacApp("/Applications/Open DrawChemistry.app".into())
        );
    }

    #[test]
    fn a_mac_app_that_cannot_be_replaced_is_not_updated() {
        for exe in [
            "/Volumes/Open DrawChemistry/Open DrawChemistry.app/Contents/MacOS/app",
            "/private/var/folders/x/AppTranslocation/ID/d/Open DrawChemistry.app/Contents/MacOS/app",
            "/usr/local/bin/app",
            "target/debug/app",
        ] {
            assert_eq!(
                detect(&environment(Os::MacOs, exe)),
                InstallFormat::Other,
                "{exe}"
            );
        }
    }

    #[test]
    fn an_unknown_os_is_not_updated() {
        let mut env = environment(Os::Other, "/opt/app");
        env.appimage = Some("/x.AppImage".into());
        env.appdir = Some("/opt".into());
        assert_eq!(detect(&env), InstallFormat::Other);
    }

    #[test]
    fn the_setup_command_line_keeps_the_install_mode() {
        assert!(setup_arguments(Scope::CurrentUser).ends_with("/CURRENTUSER"));
        assert!(setup_arguments(Scope::AllUsers).ends_with("/ALLUSERS"));
        for scope in [Scope::CurrentUser, Scope::AllUsers] {
            for flag in ["/SILENT", "/CLOSEAPPLICATIONS", "/NORESTART"] {
                assert!(setup_arguments(scope).contains(flag));
            }
        }
    }

    /// `&` after `if` would otherwise belong to it and only run on failure.
    #[test]
    fn the_app_restarts_whether_or_not_the_installer_succeeded() {
        let after_failure = SETUP_SCRIPT.find(r#"(if errorlevel 1 start "" "%ODC_RELEASE_PAGE%")"#);
        let restart = SETUP_SCRIPT.find(r#" & start "" "%ODC_APP%""#);
        assert!(after_failure.is_some() && restart > after_failure);
    }

    fn signed_download(
        format: InstallFormat,
        bytes: &[u8],
        claimed_size: u64,
    ) -> (Download, FakeTransport) {
        let name = "open-draw-chemistry-1.2.3-asset";
        let keys = SigningKey::from_bytes(&[9; 32]);
        let hex: String = keys
            .verifying_key()
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("asset");
        std::fs::write(&path, bytes).unwrap();
        let sha256: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let signature = keys.sign(signed_message(name, &sha256).as_bytes());
        let asset_url = "https://github.com/asset";
        let signature_url = "https://github.com/asset.sig";
        let transport = FakeTransport::default()
            .with(asset_url, Canned::Body(bytes.to_vec()))
            .with(signature_url, Canned::Body(signature.to_bytes().to_vec()));
        let download = Download {
            format,
            asset: Asset {
                name: name.to_owned(),
                browser_download_url: asset_url.to_owned(),
                size: claimed_size,
            },
            signature: Asset {
                name: format!("{name}.sig"),
                browser_download_url: signature_url.to_owned(),
                size: 64,
            },
            key: PublicKey::parse(&hex).unwrap(),
        };
        (download, transport)
    }

    fn prepare_download(download: &Download, transport: &FakeTransport) -> Result<Prepared, Error> {
        prepare(
            transport,
            download,
            &AtomicBool::new(false),
            &AtomicU64::new(0),
        )
    }

    fn windows_format() -> InstallFormat {
        InstallFormat::WindowsInstaller {
            scope: Scope::AllUsers,
            exe: "app.exe".into(),
        }
    }

    #[test]
    fn a_good_download_is_ready_to_install_and_its_directory_goes_when_dropped() {
        let (download, transport) = signed_download(windows_format(), b"setup", 5);
        let prepared = prepare_download(&download, &transport).unwrap();
        let Prepared::WindowsInstaller { setup, .. } = &prepared else {
            panic!("not an installer");
        };
        assert_eq!(std::fs::read(setup).unwrap(), b"setup");
        let setup = setup.clone();
        drop(prepared);
        assert!(!setup.exists());
    }

    #[test]
    fn a_tampered_download_is_not_installed() {
        let (download, mut transport) = signed_download(windows_format(), b"setup", 5);
        transport.replies.insert(
            download.asset.browser_download_url.clone(),
            Canned::Body(b"SETUP".to_vec()),
        );
        assert!(matches!(
            prepare_download(&download, &transport),
            Err(Error::Signature)
        ));
    }

    #[test]
    fn a_missing_signature_is_not_installed() {
        let (download, mut transport) = signed_download(windows_format(), b"setup", 5);
        transport.replies.insert(
            download.signature.browser_download_url.clone(),
            Canned::Status(404),
        );
        assert!(prepare_download(&download, &transport).is_err());
    }

    /// What fails verification must not have touched the AppImage it would replace.
    #[test]
    fn a_failed_appimage_download_leaves_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("App.AppImage");
        std::fs::write(&target, b"old").unwrap();
        let (download, mut transport) =
            signed_download(InstallFormat::AppImage(target.clone()), b"new", 3);
        transport.replies.insert(
            download.asset.browser_download_url.clone(),
            Canned::Body(b"bad".to_vec()),
        );
        assert!(prepare_download(&download, &transport).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn an_appimage_is_replaced_by_a_rename_keeping_its_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("App.AppImage");
        std::fs::write(&target, b"old").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let permissions = std::fs::metadata(&target).unwrap().permissions();

        let (download, transport) =
            signed_download(InstallFormat::AppImage(target.clone()), b"new", 3);
        let Prepared::AppImage { staged, target: to } =
            prepare_download(&download, &transport).unwrap()
        else {
            panic!("not an AppImage");
        };
        // Nothing has happened to the original yet, and the new file is beside it.
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert_eq!(staged.parent(), target.parent());
        assert_eq!(
            std::fs::metadata(&staged).unwrap().permissions(),
            permissions
        );

        staged.persist(&to).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions(),
            permissions
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn an_unused_staged_appimage_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("App.AppImage");
        std::fs::write(&target, b"old").unwrap();
        let source = dir.path().join("source");
        std::fs::write(&source, b"new").unwrap();
        drop(stage_file(&source, &target).unwrap());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    /// A bundle standing for the `.app` directory at `path`.
    fn bundle(path: &Path, content: &[u8]) {
        std::fs::create_dir_all(path.join("Contents")).unwrap();
        std::fs::write(path.join("Contents/app"), content).unwrap();
    }

    #[test]
    fn a_bundle_is_swapped_into_place() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("App.app");
        bundle(&target, b"old");
        let staging = stage_bundle_dir(&target).unwrap();
        bundle(&new_bundle(staging.path()), b"new");

        swap_bundle(
            &new_bundle(staging.path()),
            &target,
            &old_bundle(staging.path()),
        )
        .unwrap();
        assert_eq!(std::fs::read(target.join("Contents/app")).unwrap(), b"new");
        drop(staging);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_failed_swap_puts_the_old_bundle_back() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("App.app");
        bundle(&target, b"old");
        let missing = dir.path().join("missing.app");
        let old = dir.path().join("old.app");
        assert!(swap_bundle(&missing, &target, &old).is_err());
        assert_eq!(std::fs::read(target.join("Contents/app")).unwrap(), b"old");
        assert!(!old.exists());
    }

    #[test]
    fn stale_downloads_are_deleted_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(format!("{TEMP_PREFIX}abc"));
        let other = dir.path().join("something-else");
        std::fs::create_dir(&stale).unwrap();
        std::fs::create_dir(&other).unwrap();
        // Too young to touch.
        remove_older_than(dir.path(), Duration::from_secs(3600));
        assert!(stale.exists());
        remove_older_than(dir.path(), Duration::ZERO);
        assert!(!stale.exists());
        assert!(other.exists());
    }

    #[test]
    fn progress_follows_the_download() {
        let (download, transport) = signed_download(windows_format(), b"setup", 5);
        let progress = AtomicU64::new(0);
        prepare(&transport, &download, &AtomicBool::new(false), &progress).unwrap();
        assert_eq!(progress.load(Ordering::Relaxed), 5);
    }
}
