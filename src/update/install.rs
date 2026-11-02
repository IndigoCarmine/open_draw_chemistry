//! How this copy was installed, and installing a verified download over it.
//!
//! [`prepare`] does everything that can fail while the app is still open, downloading and
//! verifying included, and [`Prepared::apply`] does the rest as the app exits.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::time::{Duration, Instant, SystemTime};

use tempfile::{TempDir, TempPath};

use super::Error;
use super::http::{self, Transport};
use super::release::Download;
use super::signature::{self, PublicKey};

/// Starts every directory and file an update leaves in the temporary directory or beside the
/// app.
const TEMP_PREFIX: &str = ".open-draw-chemistry-update-";

/// Starts a previous version kept beside the app, which is only deleted once the app it is the
/// previous version of is back at its path (see [`remove_stale_beside`]).
const BACKUP_PREFIX: &str = ".open-draw-chemistry-update-old-";

/// How old a leftover download must be to be deleted, so that one in use by another running
/// copy of the app is left alone.
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// How long a new version gets to fail before it counts as started. A program that cannot run
/// at all is gone well within this, and the app waits it out as it exits.
const START_GRACE: Duration = Duration::from_secs(2);

/// How often [`start`] looks at the program it started.
const START_POLL: Duration = Duration::from_millis(20);

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
        verification: Verification,
        scope: Scope,
        exe: PathBuf,
        temp: TempDir,
    },
    /// `staged` is the new AppImage, beside `target`, so that a rename replaces it.
    AppImage { staged: TempPath, target: PathBuf },
    /// `staging` holds the new bundle, beside `target` for the same reason.
    MacApp { staging: TempDir, target: PathBuf },
}

/// What an asset was verified against, to do it again later.
#[derive(Debug)]
pub struct Verification {
    key: PublicKey,
    name: String,
    signature: Vec<u8>,
}

impl Verification {
    fn check(&self, path: &Path) -> Result<(), Error> {
        signature::verify_file(&self.key, &self.name, path, &self.signature)
    }
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
    let verification = Verification {
        key: download.key,
        name: download.asset.name.clone(),
        signature,
    };
    verification.check(&path)?;

    match &download.format {
        InstallFormat::WindowsInstaller { scope, exe } => Ok(Prepared::WindowsInstaller {
            setup: path,
            verification,
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

/// A free path beside `target` for a previous version of it, named so that a sweep can tell
/// which app it belongs to. It is not in a [`TempDir`]: nothing deletes it by going out of scope.
fn backup_path(target: &Path) -> Result<PathBuf, Error> {
    let unusable = || Error::Install(target.display().to_string());
    let dir = target.parent().ok_or_else(unusable)?;
    let name = target.file_name().ok_or_else(unusable)?.to_string_lossy();
    (0..1000)
        .map(|number| dir.join(format!("{BACKUP_PREFIX}{name}.{number}")))
        .find(|path| path.symlink_metadata().is_err())
        .ok_or_else(unusable)
}

/// Whether `rest`, a backup's name after [`BACKUP_PREFIX`], is that of a previous version of
/// `target`. See [`backup_path`].
fn is_backup_of(rest: &str, target: &Path) -> bool {
    let name = target.file_name().map(|name| name.to_string_lossy());
    rest.rsplit_once('.')
        .is_some_and(|(of, number)| Some(of) == name.as_deref() && number.parse::<u32>().is_ok())
}

/// Date a previous version today. Moving it kept the date of its install, and sweeps go by date:
/// it has to outlast the new version's first start, which sweeps while the old process may still
/// need it, and it gives a day to go back by hand. If this fails a sweep may just come sooner.
fn date_backup(path: &Path) {
    // A directory only opens for reading, and on Windows a file only takes a date if opened for
    // writing.
    let file = if path.is_dir() {
        File::open(path)
    } else {
        File::options().write(true).open(path)
    };
    let _ = file.and_then(|file| file.set_modified(SystemTime::now()));
}

/// Delete `path`, whatever it is. What cannot be deleted stays for a later sweep.
fn discard(path: &Path) {
    let _ = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
}

/// The two paths to tell the user about when the versions could not be sorted out.
fn stranded(one: &Path, other: &Path) -> Error {
    Error::Stranded(vec![one.to_owned(), other.to_owned()])
}

/// Put the file `new` in `target`'s place, keeping a copy of the old one at `backup`. The rename
/// is one step, so there is a file at `target` throughout. Returns where the old version is.
fn swap_file(
    steps: &dyn Steps,
    new: &Path,
    target: &Path,
    backup: &Path,
) -> Result<PathBuf, Error> {
    steps.copy_file(target, backup)?;
    if let Err(e) = steps.rename(new, target) {
        discard(backup);
        return Err(e.into());
    }
    Ok(backup.to_owned())
}

/// Replace the file `target` with `new`, and put the old one back if the new one does not start.
fn install_file(steps: &dyn Steps, new: &Path, target: &Path) -> Result<(), Error> {
    let old = swap_file(steps, new, target, &backup_path(target)?)?;
    date_backup(&old);
    let Err(error) = steps.launch(&mut Command::new(target)) else {
        return Ok(());
    };
    steps
        .rename(&old, target)
        .map_err(|_| stranded(&old, target))?;
    Err(error)
}

/// Put the bundle `new` in `target`'s place, the old one going to `backup`, and return where the
/// old one is. A bundle is a directory, so a rename cannot replace it: where the filesystem
/// cannot exchange the two there is a moment with nothing at `target`, and the old bundle waits
/// at `backup` for whatever happens then. An error other than [`Error::Stranded`] changed nothing.
fn swap_bundle(
    steps: &dyn Steps,
    new: &Path,
    target: &Path,
    backup: &Path,
) -> Result<PathBuf, Error> {
    if steps.exchange(new, target).is_ok() {
        // `new` holds the old bundle now, and the app is in place whatever happens to it.
        return Ok(match steps.rename(new, backup) {
            Ok(()) => backup.to_owned(),
            Err(_) => new.to_owned(),
        });
    }
    steps.rename(target, backup)?;
    if let Err(e) = steps.rename(new, target) {
        return match steps.rename(backup, target) {
            Ok(()) => Err(e.into()),
            Err(_) => Err(stranded(new, backup)),
        };
    }
    Ok(backup.to_owned())
}

/// Put the bundle `old` back at `target`, whose occupant failed to start.
fn restore_bundle(steps: &dyn Steps, old: &Path, target: &Path) -> Result<(), Error> {
    let failed = backup_path(target).map_err(|_| stranded(old, target))?;
    match swap_bundle(steps, old, target, &failed) {
        Ok(failed) => {
            discard(&failed);
            Ok(())
        }
        Err(Error::Stranded(paths)) => Err(Error::Stranded(paths)),
        Err(_) => Err(stranded(old, target)),
    }
}

/// Replace the bundle `target` with `new`, and put the old one back if the new one does not start.
fn install_bundle(steps: &dyn Steps, new: &Path, target: &Path) -> Result<(), Error> {
    let old = swap_bundle(steps, new, target, &backup_path(target)?)?;
    date_backup(&old);
    let Err(error) = steps.launch(Command::new("open").arg("-n").arg(target)) else {
        return Ok(());
    };
    restore_bundle(steps, &old, target)?;
    Err(error)
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

/// Start `command` and see it through `grace`: a program that fails in that time did not start.
/// One that finishes well has handed over, which is what `open` does.
fn start(command: &mut Command, grace: Duration) -> Result<(), Error> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let started = Instant::now();
    while started.elapsed() < grace {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(Error::Install(format!("{command:?}: {status}")))
            };
        }
        std::thread::sleep(START_POLL);
    }
    Ok(())
}

/// Swap what is at `a` and `b` in one step, on the filesystems that can (APFS and HFS+).
#[cfg(target_os = "macos")]
fn exchange(a: &Path, b: &Path) -> io::Result<()> {
    use std::ffi::{CString, c_char, c_int, c_uint};
    use std::os::unix::ffi::OsStrExt;

    /// From <stdio.h>.
    const RENAME_SWAP: c_uint = 2;
    unsafe extern "C" {
        fn renamex_np(from: *const c_char, to: *const c_char, flags: c_uint) -> c_int;
    }

    let (a, b) = (
        CString::new(a.as_os_str().as_bytes())?,
        CString::new(b.as_os_str().as_bytes())?,
    );
    // SAFETY: both are NUL-terminated strings that outlive the call, which does not keep them.
    if unsafe { renamex_np(a.as_ptr(), b.as_ptr(), RENAME_SWAP) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "macos"))]
fn exchange(_: &Path, _: &Path) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

/// The steps that change an install, apart so that tests can fail, or stop after, any of them.
trait Steps {
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::rename(from, to)
    }

    /// Swap what `a` and `b` hold in one step. An error has changed nothing.
    fn exchange(&self, a: &Path, b: &Path) -> io::Result<()> {
        exchange(a, b)
    }

    /// Make `to` a copy of the file `from`, without touching `from`: a second name for it where
    /// the filesystem has those, which costs nothing.
    fn copy_file(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::hard_link(from, to).or_else(|_| std::fs::copy(from, to).map(drop))
    }

    /// Start the new version with `command` and see that it does.
    fn launch(&self, command: &mut Command) -> Result<(), Error> {
        start(command, START_GRACE)
    }

    /// Start `command`, which takes it from here, and leave it running.
    fn hand_over(&self, command: &mut Command) -> Result<(), Error> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(())
    }
}

/// The real thing.
struct System;

impl Steps for System {}

impl Prepared {
    /// A verified update that nothing will ever install, for the tests of what surrounds it.
    #[cfg(test)]
    pub fn for_test() -> Self {
        Prepared::WindowsInstaller {
            setup: PathBuf::new(),
            verification: Verification {
                key: PublicKey::parse(&"11".repeat(32)).unwrap(),
                name: String::new(),
                signature: Vec::new(),
            },
            scope: Scope::CurrentUser,
            exe: PathBuf::new(),
            temp: tempfile::tempdir().unwrap(),
        }
    }

    /// Install the update and start the new version, as the app exits. `release_page` is where
    /// a Windows installer that fails later sends the user; an error here is for the caller to
    /// send them there.
    pub fn apply(self, release_page: &str) -> Result<(), Error> {
        self.apply_with(&System, release_page)
    }

    fn apply_with(self, steps: &dyn Steps, release_page: &str) -> Result<(), Error> {
        match self {
            Prepared::WindowsInstaller {
                setup,
                verification,
                scope,
                exe,
                temp,
            } => {
                // It has sat in a directory the user can write to, perhaps for hours, and what
                // runs next may be elevated.
                verification.check(&setup)?;
                steps.hand_over(&mut setup_command(
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
                let staged = staged.keep().map_err(|e| e.error)?;
                let result = install_file(steps, &staged, &target);
                if !matches!(result, Err(Error::Stranded(_))) {
                    discard(&staged);
                }
                result
            }
            Prepared::MacApp { staging, target } => {
                // From here on the only copy of the app may be in there, so only what is said
                // below deletes it.
                let staging = staging.keep();
                let result = install_bundle(steps, &new_bundle(&staging), &target);
                match &result {
                    Err(Error::Stranded(_)) => {}
                    Err(_) => discard(&staging),
                    // Empty, unless the old bundle could not be moved out; then a sweep has it.
                    Ok(()) => drop(std::fs::remove_dir(&staging)),
                }
                result
            }
        }
    }
}

/// Delete what an update left in `dir` when the app was killed mid-download.
pub fn remove_stale(dir: &Path) {
    remove_older_than(dir, STALE_AFTER, None);
}

/// The same beside what `format` replaces, where an update stages its new version and keeps the
/// previous one. Previous versions go too: this runs in a copy of the app started from there,
/// the new version having got that far.
pub fn remove_stale_beside(format: &InstallFormat) {
    if let InstallFormat::AppImage(target) | InstallFormat::MacApp(target) = format
        && let Some(dir) = target.parent()
    {
        remove_older_than(dir, STALE_AFTER, Some(target));
    }
}

/// Delete what an update left in `dir` over `age` ago. A previous version may be all that is left
/// of an app, so it only goes when `replaced` says whose it is and that app is there.
fn remove_older_than(dir: &Path, age: Duration, replaced: Option<&Path>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let ours = match name.strip_prefix(BACKUP_PREFIX) {
            Some(rest) => {
                replaced.is_some_and(|target| target.exists() && is_backup_of(rest, target))
            }
            None => name.starts_with(TEMP_PREFIX),
        };
        if !ours {
            continue;
        }
        // Not following a link, which is nothing an update leaves.
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let stale = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|elapsed| elapsed >= age);
        // Another user's, or one in use, is simply left there.
        if stale && metadata.is_dir() {
            let _ = std::fs::remove_dir_all(entry.path());
        } else if stale && metadata.is_file() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::panic::AssertUnwindSafe;
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

    /// Fails, watches or interrupts the steps of [`Prepared::apply_with`] on a real filesystem.
    #[derive(Default)]
    struct Faults {
        /// Whether the filesystem can swap two paths in one step.
        exchanges: bool,
        /// Which calls of `rename` fail, counted from 1.
        failing_renames: &'static [usize],
        /// Whether starting the new version fails.
        launch_fails: bool,
        /// Which step the process dies after, counted from 1 over all that changed the disk.
        dies_after: Option<usize>,
        renames: Cell<usize>,
        changes: Cell<usize>,
        launches: Cell<usize>,
        handed_over: Cell<usize>,
    }

    /// What a kill leaves on disk, plus `Drop`, which a kill does not get to but which is the
    /// other way this code could lose a copy. Unwinding runs it, so both are covered at once.
    struct Killed;

    impl Faults {
        fn changed(&self) {
            self.changes.set(self.changes.get() + 1);
            if self.dies_after == Some(self.changes.get()) {
                std::panic::resume_unwind(Box::new(Killed));
            }
        }
    }

    impl Steps for Faults {
        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            self.renames.set(self.renames.get() + 1);
            if self.failing_renames.contains(&self.renames.get()) {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            std::fs::rename(from, to)?;
            self.changed();
            Ok(())
        }

        fn exchange(&self, a: &Path, b: &Path) -> io::Result<()> {
            if !self.exchanges {
                return Err(io::ErrorKind::Unsupported.into());
            }
            let aside = a.with_extension("swapping");
            std::fs::rename(a, &aside)?;
            std::fs::rename(b, a)?;
            std::fs::rename(&aside, b)?;
            self.changed();
            Ok(())
        }

        fn copy_file(&self, from: &Path, to: &Path) -> io::Result<()> {
            std::fs::copy(from, to)?;
            self.changed();
            Ok(())
        }

        fn launch(&self, _command: &mut Command) -> Result<(), Error> {
            self.launches.set(self.launches.get() + 1);
            if self.launch_fails {
                return Err(Error::Install("does not start".to_owned()));
            }
            Ok(())
        }

        fn hand_over(&self, _command: &mut Command) -> Result<(), Error> {
            self.handed_over.set(self.handed_over.get() + 1);
            Ok(())
        }
    }

    /// An AppImage holding "old", with "new" staged beside it.
    fn staged_appimage() -> (TempDir, PathBuf, Prepared) {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("App.AppImage");
        std::fs::write(&target, b"old").unwrap();
        let source = dir.path().join("source");
        std::fs::write(&source, b"new").unwrap();
        let staged = stage_file(&source, &target).unwrap();
        std::fs::remove_file(&source).unwrap();
        let prepared = Prepared::AppImage {
            staged,
            target: target.clone(),
        };
        (dir, target, prepared)
    }

    /// A bundle standing for the `.app` directory at `path`.
    fn bundle(path: &Path, content: &[u8]) {
        std::fs::create_dir_all(path.join("Contents")).unwrap();
        std::fs::write(path.join("Contents/app"), content).unwrap();
    }

    /// An app bundle holding "old", with "new" staged beside it.
    fn staged_bundle() -> (TempDir, PathBuf, Prepared) {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("App.app");
        bundle(&target, b"old");
        let staging = stage_bundle_dir(&target).unwrap();
        bundle(&new_bundle(staging.path()), b"new");
        let prepared = Prepared::MacApp {
            staging,
            target: target.clone(),
        };
        (dir, target, prepared)
    }

    /// What a file or bundle at `path` holds, if it is whole.
    fn content(path: &Path) -> Option<Vec<u8>> {
        std::fs::read(path)
            .or_else(|_| std::fs::read(path.join("Contents/app")))
            .ok()
    }

    /// Every whole copy of the app that is under `dir`, wherever it is.
    fn copies(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if let Some(bytes) = content(&path) {
                found.push((path, bytes));
            } else if path.is_dir() {
                found.extend(copies(&path));
            }
        }
        found
    }

    fn holds(dir: &Path, bytes: &[u8]) -> bool {
        copies(dir).iter().any(|(_, held)| held == bytes)
    }

    fn entries(dir: &Path) -> usize {
        std::fs::read_dir(dir).unwrap().count()
    }

    /// The previous version a finished update keeps, which is outside any staging directory.
    fn backup(target: &Path) -> Option<PathBuf> {
        std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .flatten()
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(BACKUP_PREFIX)
            })
            .map(|entry| entry.path())
    }

    /// Apply `prepared`, dying after step `step`, and say whether it really got that far.
    fn apply_until_killed(prepared: Prepared, exchanges: bool, step: usize) -> bool {
        let faults = Faults {
            exchanges,
            dies_after: Some(step),
            ..Faults::default()
        };
        let died = std::panic::catch_unwind(AssertUnwindSafe(|| {
            prepared.apply_with(&faults, "page").unwrap();
        }));
        match died {
            Ok(()) => false,
            Err(reason) => {
                assert!(reason.is::<Killed>());
                true
            }
        }
    }

    #[test]
    fn an_appimage_is_replaced_keeping_its_permissions_and_the_old_version() {
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
        let prepared = prepare_download(&download, &transport).unwrap();
        let Prepared::AppImage { staged, .. } = &prepared else {
            panic!("not an AppImage");
        };
        // Nothing has happened to the original yet, and the new file is beside it.
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert_eq!(staged.parent(), target.parent());
        assert_eq!(
            std::fs::metadata(staged).unwrap().permissions(),
            permissions
        );

        let faults = Faults::default();
        prepared.apply_with(&faults, "page").unwrap();
        assert_eq!(faults.launches.get(), 1);
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions(),
            permissions
        );
        let backup = backup(&target).expect("the old version");
        assert_eq!(std::fs::read(backup).unwrap(), b"old");
        assert_eq!(entries(dir.path()), 2);
    }

    #[test]
    fn an_unused_staged_appimage_is_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("App.AppImage");
        std::fs::write(&target, b"old").unwrap();
        let source = dir.path().join("source");
        std::fs::write(&source, b"new").unwrap();
        drop(stage_file(&source, &target).unwrap());
        assert_eq!(entries(dir.path()), 2);
    }

    /// A rename over the file is one step, so there is always an AppImage to start.
    #[test]
    fn an_interrupted_appimage_update_leaves_a_whole_appimage() {
        for step in 1.. {
            let (dir, target, prepared) = staged_appimage();
            let died = apply_until_killed(prepared, false, step);
            assert!(content(&target).is_some(), "after step {step}");
            assert!(holds(dir.path(), b"old"), "after step {step}");
            if !died {
                assert!(step > 1, "the update must change something");
                break;
            }
        }
    }

    #[test]
    fn an_appimage_that_cannot_be_put_in_place_is_left_alone() {
        let (dir, target, prepared) = staged_appimage();
        let faults = Faults {
            failing_renames: &[1],
            ..Faults::default()
        };
        assert!(prepared.apply_with(&faults, "page").is_err());
        assert_eq!(faults.launches.get(), 0);
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert_eq!(entries(dir.path()), 1);
    }

    #[test]
    fn an_appimage_that_does_not_start_gives_way_to_the_old_one() {
        let (dir, target, prepared) = staged_appimage();
        let faults = Faults {
            launch_fails: true,
            ..Faults::default()
        };
        let result = prepared.apply_with(&faults, "page");
        assert!(matches!(result, Err(Error::Install(_))), "{result:?}");
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert_eq!(entries(dir.path()), 1);
    }

    #[test]
    fn a_failed_appimage_restore_keeps_both_versions_and_says_where() {
        let (dir, _target, prepared) = staged_appimage();
        let faults = Faults {
            launch_fails: true,
            failing_renames: &[2],
            ..Faults::default()
        };
        let Err(Error::Stranded(kept)) = prepared.apply_with(&faults, "page") else {
            panic!("not reported");
        };
        assert!(holds(dir.path(), b"old") && holds(dir.path(), b"new"));
        let held: Vec<_> = kept.iter().filter_map(|path| content(path)).collect();
        assert!(held.contains(&b"old".to_vec()) && held.contains(&b"new".to_vec()));
    }

    /// A rename keeps the date of the old install, and the sweep goes by date: the new version's
    /// first start must not delete what the old process would restore if that start fails.
    #[test]
    fn the_previous_version_outlives_the_first_start_of_the_new_one() {
        let (_dir, target, prepared) = staged_appimage();
        age(&target);
        prepared.apply_with(&Faults::default(), "page").unwrap();
        remove_stale_beside(&InstallFormat::AppImage(target.clone()));
        assert_eq!(content(&backup(&target).unwrap()).unwrap(), b"old");
    }

    #[test]
    fn a_bundle_is_exchanged_into_place_keeping_the_old_one() {
        let (dir, target, prepared) = staged_bundle();
        let faults = Faults {
            exchanges: true,
            ..Faults::default()
        };
        prepared.apply_with(&faults, "page").unwrap();
        assert_eq!(faults.launches.get(), 1);
        assert_eq!(content(&target).unwrap(), b"new");
        assert_eq!(content(&backup(&target).unwrap()).unwrap(), b"old");
        assert_eq!(entries(dir.path()), 2);
    }

    #[test]
    fn a_bundle_is_renamed_into_place_without_an_exchange() {
        let (dir, target, prepared) = staged_bundle();
        prepared.apply_with(&Faults::default(), "page").unwrap();
        assert_eq!(content(&target).unwrap(), b"new");
        assert_eq!(content(&backup(&target).unwrap()).unwrap(), b"old");
        assert_eq!(entries(dir.path()), 2);
    }

    /// With an exchange the app is at its path at every moment.
    #[test]
    fn an_interrupted_exchange_leaves_a_whole_app_at_its_path() {
        for step in 1.. {
            let (dir, target, prepared) = staged_bundle();
            let died = apply_until_killed(prepared, true, step);
            assert!(content(&target).is_some(), "after step {step}");
            assert!(holds(dir.path(), b"old"), "after step {step}");
            if !died {
                assert!(step > 1, "the update must change something");
                break;
            }
        }
    }

    /// Without one there is a moment with nothing at the path, and the old bundle must be
    /// somewhere no cleanup reaches, however the process ends.
    #[test]
    fn an_interrupted_rename_leaves_the_old_app_recoverable() {
        for step in 1.. {
            let (dir, _target, prepared) = staged_bundle();
            let died = apply_until_killed(prepared, false, step);
            assert!(holds(dir.path(), b"old"), "after step {step}");
            if !died {
                assert!(step > 1, "the update must change something");
                break;
            }
        }
    }

    #[test]
    fn a_failed_swap_puts_the_old_bundle_back() {
        let (dir, target, prepared) = staged_bundle();
        let faults = Faults {
            failing_renames: &[2],
            ..Faults::default()
        };
        let result = prepared.apply_with(&faults, "page");
        assert!(matches!(result, Err(Error::Io(_))), "{result:?}");
        assert_eq!(faults.launches.get(), 0);
        assert_eq!(content(&target).unwrap(), b"old");
        assert_eq!(entries(dir.path()), 1);
    }

    #[test]
    fn a_failed_rollback_keeps_both_bundles_and_says_where() {
        let (dir, target, prepared) = staged_bundle();
        let faults = Faults {
            failing_renames: &[2, 3],
            ..Faults::default()
        };
        let Err(Error::Stranded(kept)) = prepared.apply_with(&faults, "page") else {
            panic!("not reported");
        };
        assert!(content(&target).is_none());
        let held: Vec<_> = kept.iter().filter_map(|path| content(path)).collect();
        assert!(held.contains(&b"old".to_vec()) && held.contains(&b"new".to_vec()));
        // Nothing is left to delete either of them.
        assert!(holds(dir.path(), b"old") && holds(dir.path(), b"new"));
    }

    /// The exchange worked and the app is in place: the old bundle stays where it ended up.
    #[test]
    fn an_exchanged_bundle_that_cannot_be_moved_out_of_staging_is_kept_there() {
        let (dir, target, prepared) = staged_bundle();
        let faults = Faults {
            exchanges: true,
            failing_renames: &[1],
            ..Faults::default()
        };
        prepared.apply_with(&faults, "page").unwrap();
        assert_eq!(content(&target).unwrap(), b"new");
        assert!(holds(dir.path(), b"old"));
    }

    #[test]
    fn a_bundle_that_does_not_start_gives_way_to_the_old_one() {
        for exchanges in [true, false] {
            let (dir, target, prepared) = staged_bundle();
            let faults = Faults {
                exchanges,
                launch_fails: true,
                ..Faults::default()
            };
            let result = prepared.apply_with(&faults, "page");
            assert!(matches!(result, Err(Error::Install(_))), "{result:?}");
            assert_eq!(content(&target).unwrap(), b"old", "exchanges: {exchanges}");
            assert_eq!(entries(dir.path()), 1, "exchanges: {exchanges}");
        }
    }

    /// Whichever step of putting the old bundle back fails, neither version is lost.
    #[test]
    fn a_failed_bundle_restore_keeps_both_and_says_where() {
        // Target to backup and staged to target have gone well; then, of the way back, moving
        // the failed version aside, or that and moving the old one in and undoing it.
        for failing in [&[3][..], &[4, 5][..]] {
            let (dir, _target, prepared) = staged_bundle();
            let faults = Faults {
                launch_fails: true,
                failing_renames: failing,
                ..Faults::default()
            };
            let Err(Error::Stranded(kept)) = prepared.apply_with(&faults, "page") else {
                panic!("not reported with {failing:?}");
            };
            let held: Vec<_> = kept.iter().filter_map(|path| content(path)).collect();
            assert!(
                held.contains(&b"old".to_vec()) && held.contains(&b"new".to_vec()),
                "{failing:?}: {held:?}"
            );
            assert!(holds(dir.path(), b"old") && holds(dir.path(), b"new"));
        }
    }

    /// `cmd` or `sh` running `script`.
    fn shell(script: &str) -> Command {
        let (program, flag) = if cfg!(windows) {
            ("cmd", "/C")
        } else {
            ("sh", "-c")
        };
        let mut command = Command::new(program);
        command.arg(flag).arg(script);
        command
    }

    #[test]
    fn a_program_that_fails_at_once_has_not_started() {
        let grace = Duration::from_secs(10);
        assert!(matches!(
            start(&mut shell("exit 3"), grace),
            Err(Error::Install(_))
        ));
        assert!(matches!(
            start(&mut Command::new("no-such-program-at-all"), grace),
            Err(Error::Io(_))
        ));
    }

    /// `open` hands over to the system and leaves.
    #[test]
    fn a_program_that_finishes_well_has_started() {
        start(&mut shell("exit 0"), Duration::from_secs(10)).unwrap();
    }

    #[test]
    fn a_program_still_running_after_the_grace_period_has_started() {
        let waits = if cfg!(windows) {
            "ping -n 3 127.0.0.1"
        } else {
            "sleep 2"
        };
        let started = Instant::now();
        start(&mut shell(waits), Duration::from_millis(50)).unwrap();
        assert!(started.elapsed() < Duration::from_millis(1500));
    }

    /// Where hard links cannot be made, as on a FAT stick, the backup is a copy.
    #[test]
    fn the_backup_of_a_file_is_a_copy_of_it() {
        let dir = tempfile::tempdir().unwrap();
        let (from, to) = (dir.path().join("from"), dir.path().join("to"));
        std::fs::write(&from, b"old").unwrap();
        System.copy_file(&from, &to).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"old");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_system_exchanges_two_directories() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.app"), dir.path().join("b.app"));
        bundle(&a, b"a");
        bundle(&b, b"b");
        System.exchange(&a, &b).unwrap();
        assert_eq!(content(&a).unwrap(), b"b");
        assert_eq!(content(&b).unwrap(), b"a");
    }

    /// A Yes hours ago must not hand the elevated installer whatever is in the temporary
    /// directory now: anything running as the user can write there.
    #[test]
    fn a_setup_changed_after_its_verification_is_not_started() {
        let (download, transport) = signed_download(windows_format(), b"setup", 5);
        let prepared = prepare_download(&download, &transport).unwrap();
        let Prepared::WindowsInstaller { setup, .. } = &prepared else {
            panic!("not an installer");
        };
        std::fs::write(setup, b"SETUP").unwrap();
        let faults = Faults::default();
        let result = prepared.apply_with(&faults, "page");
        assert!(matches!(result, Err(Error::Signature)), "{result:?}");
        assert_eq!(faults.handed_over.get(), 0);
    }

    #[test]
    fn a_missing_setup_is_not_started() {
        let (download, transport) = signed_download(windows_format(), b"setup", 5);
        let prepared = prepare_download(&download, &transport).unwrap();
        let Prepared::WindowsInstaller { setup, .. } = &prepared else {
            panic!("not an installer");
        };
        std::fs::remove_file(setup).unwrap();
        let faults = Faults::default();
        assert!(prepared.apply_with(&faults, "page").is_err());
        assert_eq!(faults.handed_over.get(), 0);
    }

    #[test]
    fn an_unchanged_setup_is_started() {
        let (download, transport) = signed_download(windows_format(), b"setup", 5);
        let prepared = prepare_download(&download, &transport).unwrap();
        let Prepared::WindowsInstaller { setup, .. } = &prepared else {
            panic!("not an installer");
        };
        let directory = setup.parent().unwrap().to_owned();
        let faults = Faults::default();
        prepared.apply_with(&faults, "page").unwrap();
        assert_eq!(faults.handed_over.get(), 1);
        // The script deletes it, in real life.
        assert!(directory.exists());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn stale_downloads_are_deleted_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(format!("{TEMP_PREFIX}abc"));
        let other = dir.path().join("something-else");
        std::fs::create_dir(&stale).unwrap();
        std::fs::create_dir(&other).unwrap();
        // Too young to touch.
        remove_older_than(dir.path(), Duration::from_secs(3600), None);
        assert!(stale.exists());
        remove_older_than(dir.path(), Duration::ZERO, None);
        assert!(!stale.exists());
        assert!(other.exists());
    }

    #[test]
    fn stale_files_are_deleted_too() {
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join(format!("{TEMP_PREFIX}abc"));
        let other = dir.path().join("something-else");
        std::fs::write(&stale, b"new").unwrap();
        std::fs::write(&other, b"mine").unwrap();
        remove_older_than(dir.path(), Duration::from_secs(3600), None);
        assert!(stale.exists());
        remove_older_than(dir.path(), Duration::ZERO, None);
        assert!(!stale.exists());
        assert!(other.exists());
    }

    /// Give `path` the date of two days ago.
    fn age(path: &Path) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - 2 * STALE_AFTER)
            .unwrap();
    }

    #[test]
    fn what_an_update_left_beside_the_app_is_swept() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("App.AppImage");
        let staged = dir.path().join(format!("{TEMP_PREFIX}abc"));
        let mine = dir.path().join("notes.txt");
        for file in [&target, &staged, &mine] {
            std::fs::write(file, b"x").unwrap();
            age(file);
        }
        remove_stale_beside(&InstallFormat::AppImage(target.clone()));
        assert!(!staged.exists());
        assert!(target.exists() && mine.exists());
    }

    /// What is young may be in use by another running copy.
    #[test]
    fn what_was_left_today_stays() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("App.AppImage");
        let staged = dir.path().join(format!("{TEMP_PREFIX}abc"));
        std::fs::write(&target, b"x").unwrap();
        std::fs::write(&staged, b"x").unwrap();
        remove_stale_beside(&InstallFormat::AppImage(target));
        assert!(staged.exists());
    }

    #[test]
    fn formats_that_stage_nothing_beside_the_app_sweep_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("app.exe");
        let staged = dir.path().join(format!("{TEMP_PREFIX}abc"));
        std::fs::write(&staged, b"x").unwrap();
        age(&staged);
        for format in [
            InstallFormat::Other,
            InstallFormat::WindowsInstaller {
                scope: Scope::CurrentUser,
                exe,
            },
        ] {
            remove_stale_beside(&format);
            assert!(staged.exists());
        }
    }

    /// The new version got this far, so the previous one has done its job.
    #[test]
    fn the_previous_version_goes_once_the_app_is_back_at_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("App.AppImage");
        std::fs::write(&target, b"new").unwrap();
        let backup = dir.path().join(format!("{BACKUP_PREFIX}App.AppImage.0"));
        std::fs::write(&backup, b"old").unwrap();
        age(&backup);
        remove_stale_beside(&InstallFormat::AppImage(target));
        assert!(!backup.exists());
    }

    /// It may be the only copy of that app left, and it is not this one's to delete.
    #[test]
    fn the_previous_version_of_another_app_stays() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("App.AppImage");
        std::fs::write(&target, b"new").unwrap();
        let other = dir.path().join(format!("{BACKUP_PREFIX}Other.AppImage.0"));
        let unnumbered = dir.path().join(format!("{BACKUP_PREFIX}App.AppImage"));
        for file in [&other, &unnumbered] {
            std::fs::write(file, b"old").unwrap();
            age(file);
        }
        remove_stale_beside(&InstallFormat::AppImage(target));
        assert!(other.exists() && unnumbered.exists());
    }

    /// `remove_stale` also runs on the temporary directory, where an AppImage may be kept.
    #[test]
    fn a_plain_sweep_leaves_previous_versions_alone() {
        let dir = tempfile::tempdir().unwrap();
        let backup = dir.path().join(format!("{BACKUP_PREFIX}App.AppImage.0"));
        std::fs::write(&backup, b"old").unwrap();
        remove_older_than(dir.path(), Duration::ZERO, None);
        assert!(backup.exists());
    }

    #[test]
    fn with_the_app_missing_its_previous_version_stays() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("App.AppImage");
        let backup = dir.path().join(format!("{BACKUP_PREFIX}App.AppImage.0"));
        std::fs::write(&backup, b"old").unwrap();
        age(&backup);
        remove_stale_beside(&InstallFormat::AppImage(target));
        assert!(backup.exists());
    }

    #[test]
    fn a_new_backup_does_not_take_the_name_of_an_older_one() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("App.AppImage");
        let first = backup_path(&target).unwrap();
        std::fs::write(&first, b"old").unwrap();
        assert_ne!(backup_path(&target).unwrap(), first);
    }

    #[test]
    fn progress_follows_the_download() {
        let (download, transport) = signed_download(windows_format(), b"setup", 5);
        let progress = AtomicU64::new(0);
        prepare(&transport, &download, &AtomicBool::new(false), &progress).unwrap();
        assert_eq!(progress.load(Ordering::Relaxed), 5);
    }
}
