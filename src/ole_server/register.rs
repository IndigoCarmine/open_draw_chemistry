//! Registering the OLE server for the current user (`--register-ole` / `--unregister-ole`).
//!
//! Everything goes under `HKCU\Software\Classes`, so no administrator rights are needed:
//!
//! - `CLSID\{ours}`: the server (`LocalServer32`), the default handler (`InprocHandler32`),
//!   ProgIDs, `Insertable`, `DefaultIcon`, `MiscStatus`, the verbs and the short user types;
//! - the ProgIDs `OpenDrawChemistry.Document.1` and `OpenDrawChemistry.Document`;
//! - only when ChemDraw is **not** installed: `CLSID\{ChemDraw}\TreatAs = {ours}`, so the
//!   ChemDraw objects in Office documents open here. Where ChemDraw is installed nothing of
//!   ChemDraw's is written (a per-user `TreatAs` would override ChemDraw's own registration).
//!
//! What was created is recorded in our class key (value [`RECORD_VALUE`]) and unregistering
//! removes exactly that. The registry location is a [`Hive`], so the tests run against a
//! scratch key and never touch the real classes.

use std::path::Path;

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
    KEY_WRITE, REG_CREATE_KEY_DISPOSITION, REG_CREATED_NEW_KEY, REG_MULTI_SZ,
    REG_OPTION_NON_VOLATILE, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW,
    RegDeleteKeyW, RegDeleteTreeW, RegOpenKeyExW, RegQueryInfoKeyW, RegQueryValueExW,
    RegSetValueExW,
};
use windows::core::{HSTRING, PCWSTR};

use super::{CHEMDRAW_CLSID, CHEMDRAW_PROG_IDS, CLSID, PROG_ID, PROG_ID_CURRENT, guid_string};

/// The value in our class key listing the keys registration created.
pub const RECORD_VALUE: &str = "OpenDrawChemistry.Registered";

/// Where classes are read from and written to.
#[derive(Debug, Clone)]
pub struct Hive {
    /// Read for what is installed (the merged `HKEY_CLASSES_ROOT` in real use).
    read_root: HKEY,
    read_path: String,
    /// Written to (`HKCU\Software\Classes` in real use).
    write_root: HKEY,
    write_path: String,
}

impl Hive {
    /// The current user's classes, checked against everything registered on the PC.
    pub fn current_user() -> Self {
        Self {
            read_root: HKEY_CLASSES_ROOT,
            read_path: String::new(),
            write_root: HKEY_CURRENT_USER,
            write_path: r"Software\Classes".to_owned(),
        }
    }

    /// A stand-in for the classes at `HKCU\<path>`, both read and written (tests).
    pub fn scratch(path: &str) -> Self {
        Self {
            read_root: HKEY_CURRENT_USER,
            read_path: path.to_owned(),
            write_root: HKEY_CURRENT_USER,
            write_path: path.to_owned(),
        }
    }

    fn read(&self, sub: &str) -> String {
        join(&self.read_path, sub)
    }

    fn write(&self, sub: &str) -> String {
        join(&self.write_path, sub)
    }
}

fn join(base: &str, sub: &str) -> String {
    match (base.is_empty(), sub.is_empty()) {
        (true, _) => sub.to_owned(),
        (_, true) => base.to_owned(),
        _ => format!(r"{base}\{sub}"),
    }
}

/// An open registry key, closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the handle came from RegOpenKeyExW/RegCreateKeyExW and is closed once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn check(e: WIN32_ERROR) -> windows::core::Result<()> {
    if e == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(windows::core::Error::from_hresult(e.to_hresult()))
    }
}

fn open(root: HKEY, path: &str, access: REG_SAM_FLAGS) -> Option<Key> {
    let mut key = HKEY::default();
    // SAFETY: valid root, a NUL-terminated path, and an out-pointer to a local.
    let e = unsafe { RegOpenKeyExW(root, &HSTRING::from(path), None, access, &mut key) };
    (e == ERROR_SUCCESS).then_some(Key(key))
}

/// Create (or open) `path`; whether it was newly created.
fn create(root: HKEY, path: &str) -> windows::core::Result<(Key, bool)> {
    let mut key = HKEY::default();
    let mut disposition = REG_CREATE_KEY_DISPOSITION::default();
    // SAFETY: as in `open`; the disposition out-pointer is a local.
    check(unsafe {
        RegCreateKeyExW(
            root,
            &HSTRING::from(path),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            None,
            &mut key,
            Some(&mut disposition),
        )
    })?;
    Ok((Key(key), disposition == REG_CREATED_NEW_KEY))
}

fn set(key: &Key, name: &str, kind: REG_VALUE_TYPE, data: &[u16]) -> windows::core::Result<()> {
    // SAFETY: `data` is a valid buffer of the given length in bytes.
    let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), data.len() * 2) };
    check(unsafe { RegSetValueExW(key.0, &HSTRING::from(name), None, kind, Some(bytes)) })
}

fn set_str(key: &Key, name: &str, value: &str) -> windows::core::Result<()> {
    let data: Vec<u16> = value.encode_utf16().chain([0]).collect();
    set(key, name, REG_SZ, &data)
}

fn set_multi(key: &Key, name: &str, values: &[String]) -> windows::core::Result<()> {
    let mut data: Vec<u16> = Vec::new();
    for v in values {
        data.extend(v.encode_utf16());
        data.push(0);
    }
    data.push(0);
    set(key, name, REG_MULTI_SZ, &data)
}

/// A string (or multi-string, NUL-separated) value.
fn get(key: &Key, name: &str) -> Option<String> {
    let name = HSTRING::from(name);
    let mut size = 0u32;
    // SAFETY: size query with no buffer, then a read into a buffer of that size.
    unsafe {
        if RegQueryValueExW(key.0, &name, None, None, None, Some(&mut size)) != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        if RegQueryValueExW(
            key.0,
            &name,
            None,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        ) != ERROR_SUCCESS
        {
            return None;
        }
        buf.truncate(size as usize / 2);
        while buf.last() == Some(&0) {
            buf.pop();
        }
        Some(String::from_utf16_lossy(&buf))
    }
}

fn delete_tree(root: HKEY, path: &str) -> windows::core::Result<()> {
    let path = HSTRING::from(path);
    // SAFETY: valid root and path. RegDeleteTreeW empties the key; RegDeleteKeyW removes it.
    unsafe {
        let e = RegDeleteTreeW(root, &path);
        if e != ERROR_FILE_NOT_FOUND {
            check(e)?;
        }
        let e = RegDeleteKeyW(root, &path);
        if e != ERROR_FILE_NOT_FOUND {
            check(e)?;
        }
    }
    Ok(())
}

fn is_empty(root: HKEY, path: &str) -> bool {
    let Some(key) = open(root, path, KEY_READ) else {
        return false;
    };
    let (mut subkeys, mut values) = (0u32, 0u32);
    // SAFETY: counts into locals.
    let e = unsafe {
        RegQueryInfoKeyW(
            key.0,
            None,
            None,
            None,
            Some(&mut subkeys),
            None,
            None,
            Some(&mut values),
            None,
            None,
            None,
            None,
        )
    };
    e == ERROR_SUCCESS && subkeys == 0 && values == 0
}

/// Whether ChemDraw is installed: its class has a local server (in either registry view), or
/// one of its ProgIDs is registered. Only keys ChemDraw writes are looked at, never the
/// `TreatAs` this app may have added.
pub fn chemdraw_installed(hive: &Hive) -> bool {
    let class = format!(r"CLSID\{}", guid_string(CHEMDRAW_CLSID));
    let server = [r"LocalServer32", r"LocalServer"].iter().any(|s| {
        [KEY_WOW64_64KEY, KEY_WOW64_32KEY].iter().any(|&view| {
            open(
                hive.read_root,
                &hive.read(&format!(r"{class}\{s}")),
                KEY_READ | view,
            )
            .is_some()
        })
    });
    server
        || CHEMDRAW_PROG_IDS.iter().any(|id| {
            open(
                hive.read_root,
                &hive.read(&format!(r"{id}\CLSID")),
                KEY_READ,
            )
            .is_some()
        })
}

/// What [`register`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered {
    /// ChemDraw's class now opens in this app.
    pub treat_as: bool,
    /// A `TreatAs` from an earlier registration was removed because ChemDraw is now installed.
    pub removed_stale_treat_as: bool,
    /// The keys created, relative to the classes root.
    pub created: Vec<String>,
}

fn our_class() -> String {
    format!(r"CLSID\{}", guid_string(CLSID))
}

fn chemdraw_class() -> String {
    format!(r"CLSID\{}", guid_string(CHEMDRAW_CLSID))
}

/// Register the server at `exe` (its `LocalServer32` command: the quoted path, then `args`).
pub fn register(hive: &Hive, exe: &Path, args: &str) -> windows::core::Result<Registered> {
    let ours = guid_string(CLSID);
    let class = our_class();
    let exe_s = exe.display().to_string();
    let mut created: Vec<String> = read_record(hive);
    let mut note = |path: &str, new: bool| {
        if new && !created.iter().any(|c| c == path) {
            created.push(path.to_owned());
        }
    };

    let (key, new) = create(hive.write_root, &hive.write(&class))?;
    note(&class, new);
    set_str(&key, "", super::user_type())?;
    let sub = |name: &str, value: &str| -> windows::core::Result<()> {
        let (k, _) = create(hive.write_root, &hive.write(&format!(r"{class}\{name}")))?;
        set_str(&k, "", value)
    };
    let command = if args.is_empty() {
        format!("\"{exe_s}\"")
    } else {
        format!("\"{exe_s}\" {args}")
    };
    sub("LocalServer32", &command)?;
    sub("InprocHandler32", "ole32.dll")?;
    sub("ProgID", PROG_ID)?;
    sub("VersionIndependentProgID", PROG_ID_CURRENT)?;
    sub("Insertable", "")?;
    sub("DefaultIcon", &format!("{exe_s},0"))?;
    // OLEMISC_CANTLINKINSIDE: no links into the drawing.
    sub("MiscStatus", "16")?;
    // MF_STRING (0), OLEVERBATTRIB_ONCONTAINERMENU (2).
    sub(r"Verb\0", &format!("{},0,2", t!("ole.server.verb_edit")))?;
    sub(r"Verb\1", &format!("{},0,2", t!("ole.server.verb_open")))?;
    sub(r"AuxUserType\2", super::user_type_short())?;
    sub(r"AuxUserType\3", crate::APP_NAME)?;

    for (prog_id, versioned) in [(PROG_ID, true), (PROG_ID_CURRENT, false)] {
        let (k, new) = create(hive.write_root, &hive.write(prog_id))?;
        note(prog_id, new);
        set_str(&k, "", super::user_type())?;
        let (c, _) = create(hive.write_root, &hive.write(&format!(r"{prog_id}\CLSID")))?;
        set_str(&c, "", &ours)?;
        if versioned {
            create(
                hive.write_root,
                &hive.write(&format!(r"{prog_id}\Insertable")),
            )?;
        } else {
            let (v, _) = create(hive.write_root, &hive.write(&format!(r"{prog_id}\CurVer")))?;
            set_str(&v, "", PROG_ID)?;
        }
    }

    let treat_as_path = format!(r"{}\TreatAs", chemdraw_class());
    let mut removed_stale_treat_as = false;
    let treat_as = if chemdraw_installed(hive) {
        // Undo a TreatAs written before ChemDraw was installed, so ChemDraw gets its objects.
        if treat_as_is_ours(hive) {
            remove_treat_as(hive)?;
            created.retain(|c| *c != treat_as_path && *c != chemdraw_class());
            removed_stale_treat_as = true;
        }
        false
    } else {
        let cd_class = chemdraw_class();
        let (_, new) = create(hive.write_root, &hive.write(&cd_class))?;
        note(&cd_class, new);
        let (k, new) = create(hive.write_root, &hive.write(&treat_as_path))?;
        note(&treat_as_path, new);
        set_str(&k, "", &ours)?;
        true
    };

    let (key, _) = create(hive.write_root, &hive.write(&class))?;
    set_multi(&key, RECORD_VALUE, &created)?;
    Ok(Registered {
        treat_as,
        removed_stale_treat_as,
        created,
    })
}

fn read_record(hive: &Hive) -> Vec<String> {
    open(hive.write_root, &hive.write(&our_class()), KEY_READ)
        .and_then(|k| get(&k, RECORD_VALUE))
        .map(|s| {
            s.split('\0')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn treat_as_is_ours(hive: &Hive) -> bool {
    open(
        hive.write_root,
        &hive.write(&format!(r"{}\TreatAs", chemdraw_class())),
        KEY_READ,
    )
    .and_then(|k| get(&k, ""))
    .is_some_and(|v| v.eq_ignore_ascii_case(&guid_string(CLSID)))
}

/// Remove our `TreatAs` from ChemDraw's class, and the class key when we left it empty.
fn remove_treat_as(hive: &Hive) -> windows::core::Result<()> {
    if treat_as_is_ours(hive) {
        delete_tree(
            hive.write_root,
            &hive.write(&format!(r"{}\TreatAs", chemdraw_class())),
        )?;
    }
    let class = hive.write(&chemdraw_class());
    if is_empty(hive.write_root, &class) {
        delete_tree(hive.write_root, &class)?;
    }
    Ok(())
}

/// Remove what [`register`] created. Our own class and ProgIDs are removed even without a
/// record (they are only ever ours); ChemDraw's class only loses a `TreatAs` pointing here.
pub fn unregister(hive: &Hive) -> windows::core::Result<()> {
    let record = read_record(hive);
    let treat_as_path = format!(r"{}\TreatAs", chemdraw_class());
    if record.contains(&treat_as_path) || treat_as_is_ours(hive) {
        remove_treat_as(hive)?;
    }
    for key in [PROG_ID, PROG_ID_CURRENT] {
        // A ProgID key that existed before us is left, minus nothing: only remove it when it
        // was ours (recorded) or still names our class.
        let names_us = open(
            hive.write_root,
            &hive.write(&format!(r"{key}\CLSID")),
            KEY_READ,
        )
        .and_then(|k| get(&k, ""))
        .is_some_and(|v| v.eq_ignore_ascii_case(&guid_string(CLSID)));
        if record.iter().any(|c| c == key) || names_us {
            delete_tree(hive.write_root, &hive.write(key))?;
        }
    }
    delete_tree(hive.write_root, &hive.write(&our_class()))
}

/// The command for `--register-ole`: register this executable for the current user.
pub fn register_current_exe(extra_args: &str) -> Result<Registered, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    register(&Hive::current_user(), &exe, extra_args).map_err(|e| e.to_string())
}

/// The command for `--unregister-ole`.
pub fn unregister_current_user() -> Result<(), String> {
    unregister(&Hive::current_user()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch hive under `HKCU\Software\OpenDrawChemistryTest\<name>-<pid>`, removed on drop.
    struct Scratch(Hive, String);

    impl Scratch {
        fn new(name: &str) -> Self {
            let path = format!(
                r"Software\OpenDrawChemistryTest\{name}-{}",
                std::process::id()
            );
            // As in the real classes root, CLSID exists before us.
            create(HKEY_CURRENT_USER, &join(&path, "CLSID")).unwrap();
            Scratch(Hive::scratch(&path), path)
        }

        fn exists(&self, sub: &str) -> bool {
            open(HKEY_CURRENT_USER, &join(&self.1, sub), KEY_READ).is_some()
        }

        fn value(&self, sub: &str, name: &str) -> Option<String> {
            open(HKEY_CURRENT_USER, &join(&self.1, sub), KEY_READ).and_then(|k| get(&k, name))
        }

        /// Pretend ChemDraw is installed, as its installer registers it.
        fn install_chemdraw(&self) {
            let (k, _) = create(
                HKEY_CURRENT_USER,
                &join(&self.1, &format!(r"{}\LocalServer32", chemdraw_class())),
            )
            .unwrap();
            set_str(&k, "", r"C:\ChemDraw\ChemDraw.exe").unwrap();
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = delete_tree(HKEY_CURRENT_USER, &self.1);
            // Only removed once empty: a concurrent test's key keeps it.
            // SAFETY: valid root and path.
            unsafe {
                let _ = RegDeleteKeyW(
                    HKEY_CURRENT_USER,
                    &HSTRING::from(r"Software\OpenDrawChemistryTest"),
                );
            }
        }
    }

    #[test]
    fn chemdraw_is_detected_by_its_server_or_prog_id() {
        let s = Scratch::new("detect");
        assert!(!chemdraw_installed(&s.0));
        s.install_chemdraw();
        assert!(chemdraw_installed(&s.0));

        let t = Scratch::new("detect-progid");
        create(
            HKEY_CURRENT_USER,
            &join(&t.1, r"ChemDraw_x64.Document.6.0\CLSID"),
        )
        .unwrap();
        assert!(chemdraw_installed(&t.0));
    }

    #[test]
    fn our_own_treat_as_does_not_count_as_chemdraw() {
        let s = Scratch::new("own-treatas");
        register(&s.0, Path::new(r"C:\app\odc.exe"), "").unwrap();
        assert!(!chemdraw_installed(&s.0));
    }

    #[test]
    fn without_chemdraw_its_class_is_treated_as_ours_and_unregister_restores() {
        let s = Scratch::new("no-chemdraw");
        let r = register(&s.0, Path::new(r"C:\app\odc.exe"), "").unwrap();
        assert!(r.treat_as);
        let class = our_class();
        assert_eq!(
            s.value(&format!(r"{class}\LocalServer32"), "").as_deref(),
            Some(r#""C:\app\odc.exe""#)
        );
        assert_eq!(
            s.value(&format!(r"{}\TreatAs", chemdraw_class()), "")
                .as_deref(),
            Some(guid_string(CLSID).as_str())
        );
        assert_eq!(
            s.value(&format!(r"{PROG_ID}\CLSID"), "").as_deref(),
            Some(guid_string(CLSID).as_str())
        );
        assert!(s.exists(&format!(r"{class}\Insertable")));
        assert_eq!(
            s.value(&format!(r"{class}\Verb\0"), "").as_deref(),
            Some("&Edit,0,2")
        );

        // Registering twice is harmless and still unregisters fully.
        register(&s.0, Path::new(r"C:\app\odc.exe"), "").unwrap();
        unregister(&s.0).unwrap();
        for key in [
            class,
            chemdraw_class(),
            PROG_ID.into(),
            PROG_ID_CURRENT.into(),
        ] {
            assert!(!s.exists(&key), "{key} left behind");
        }
        assert!(
            is_empty(HKEY_CURRENT_USER, &join(&s.1, "CLSID")),
            "nothing is left among the classes"
        );
    }

    #[test]
    fn with_chemdraw_nothing_of_chemdraws_is_touched() {
        let s = Scratch::new("with-chemdraw");
        s.install_chemdraw();
        let r = register(&s.0, Path::new(r"C:\app\odc.exe"), "--x").unwrap();
        assert!(!r.treat_as);
        assert!(!s.exists(&format!(r"{}\TreatAs", chemdraw_class())));
        assert_eq!(
            s.value(&format!(r"{}\LocalServer32", our_class()), "")
                .as_deref(),
            Some(r#""C:\app\odc.exe" --x"#)
        );
        unregister(&s.0).unwrap();
        // ChemDraw's own registration survives.
        assert!(s.exists(&format!(r"{}\LocalServer32", chemdraw_class())));
        assert!(!s.exists(&our_class()));
    }

    #[test]
    fn a_treat_as_from_before_chemdraw_was_installed_is_undone() {
        let s = Scratch::new("stale");
        assert!(register(&s.0, Path::new(r"C:\a.exe"), "").unwrap().treat_as);
        s.install_chemdraw();
        let r = register(&s.0, Path::new(r"C:\a.exe"), "").unwrap();
        assert!(r.removed_stale_treat_as);
        assert!(!s.exists(&format!(r"{}\TreatAs", chemdraw_class())));
        assert!(s.exists(&format!(r"{}\LocalServer32", chemdraw_class())));
    }
}
