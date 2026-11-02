//! What GitHub says about the latest release, and what to do about it.

use semver::Version;
use serde::Deserialize;
use url::Url;

use super::install::InstallFormat;
use super::signature::PublicKey;
use super::{Error, LATEST_RELEASE_PAGE, http};

/// The repository's Release pages and assets, under `https://github.com`.
const RELEASES_PATH: &str = "/IndigoCarmine/open_draw_chemistry/releases";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

/// The fields of GitHub's description of a release that are used here.
#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn parse(json: &[u8]) -> Result<Self, Error> {
        serde_json::from_slice(json).map_err(|e| Error::Malformed(e.to_string()))
    }

    /// The release's own page when GitHub's answer names one of this repository, or else the
    /// page of the latest release. What this returns is what gets opened in the browser, so it
    /// is the URL as parsed, in which anything unusual is percent-encoded, not the text received.
    pub fn page(&self) -> String {
        repository_url(&self.html_url, RELEASES_PATH)
            .map_or_else(|| LATEST_RELEASE_PAGE.to_owned(), String::from)
    }
}

/// `url`, when it is on github.com, under `path` of this repository.
fn repository_url(url: &str, path: &str) -> Option<Url> {
    let url = Url::parse(url).ok()?;
    let under_path = url
        .path()
        .strip_prefix(path)
        .is_some_and(|rest| rest.starts_with('/'));
    (http::is_allowed(&url) && url.host_str() == Some("github.com") && under_path).then_some(url)
}

/// Whether a build of version `current` looks for updates at all: a pre-release, which is what
/// every local `-dev` build is, does not.
pub fn is_updatable(current: &str) -> bool {
    Version::parse(current).is_ok_and(|current| current.pre.is_empty())
}

/// Whether `latest` is newer than `current`, in semver's order: `0.3.0-dev` is before `0.3.0`,
/// and build metadata (`1.2.3+x`) does not count.
pub fn is_newer(current: &Version, latest: &Version) -> bool {
    latest.cmp_precedence(current).is_gt()
}

/// The version of `release`, when a build of version `current` should be offered it. A
/// pre-release tag is not offered, whatever its number.
pub fn newer_version(current: &str, release: &Release) -> Option<Version> {
    if !is_updatable(current) {
        return None;
    }
    let current = Version::parse(current).ok()?;
    let tag = release.tag_name.strip_prefix('v');
    let latest = Version::parse(tag.unwrap_or(&release.tag_name)).ok()?;
    (latest.pre.is_empty() && is_newer(&current, &latest)).then_some(latest)
}

/// The name a release gives the asset for `format`, as .github/workflows/release.yml builds it.
/// The name holds the version, which is what the signature is bound to.
fn asset_name(format: &InstallFormat, version: &Version) -> Option<String> {
    let suffix = match format {
        InstallFormat::WindowsInstaller { .. } => "windows-x64-setup.exe",
        InstallFormat::AppImage(_) => "linux-x86_64.AppImage",
        InstallFormat::MacApp(_) => "macos-universal.dmg",
        InstallFormat::Other => return None,
    };
    Some(format!("open-draw-chemistry-{version}-{suffix}"))
}

/// Everything needed to fetch and verify an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub format: InstallFormat,
    pub asset: Asset,
    /// The asset's detached signature, `<asset>.sig`.
    pub signature: Asset,
    pub key: PublicKey,
}

/// What answering Yes does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Install(Download),
    ReleasePage(Why),
}

/// Why an update cannot be installed automatically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// The portable zip, a .deb, or anything not recognised.
    UnsupportedFormat,
    /// The app holds the placeholder rather than a real public key.
    NoSigningKey,
    /// The release has no such asset, or no signature for it (say it was built without the
    /// signing secret).
    NoAsset,
}

/// What Yes will do about `release`, which is of version `version`. Nothing here fails: all that
/// cannot be installed automatically goes to the Release page.
pub fn plan(
    format: InstallFormat,
    release: &Release,
    version: &Version,
    key: Option<PublicKey>,
) -> Plan {
    let Some(name) = asset_name(&format, version) else {
        return Plan::ReleasePage(Why::UnsupportedFormat);
    };
    let Some(key) = key else {
        return Plan::ReleasePage(Why::NoSigningKey);
    };
    let find = |name: &str| {
        release.assets.iter().find(|asset| {
            asset.name == name
                && repository_url(
                    &asset.browser_download_url,
                    &format!("{RELEASES_PATH}/download"),
                )
                .is_some()
        })
    };
    match (find(&name), find(&format!("{name}.sig"))) {
        (Some(asset), Some(signature)) => Plan::Install(Download {
            format,
            asset: asset.clone(),
            signature: signature.clone(),
            key,
        }),
        _ => Plan::ReleasePage(Why::NoAsset),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::install::Scope;
    use super::*;

    const ASSET_BASE: &str =
        "https://github.com/IndigoCarmine/open_draw_chemistry/releases/download/v1.2.3";

    fn key() -> PublicKey {
        PublicKey::parse(&"11".repeat(32)).unwrap()
    }

    fn asset(name: &str) -> Asset {
        Asset {
            name: name.to_owned(),
            browser_download_url: format!("{ASSET_BASE}/{name}"),
            size: 1000,
        }
    }

    fn release(names: &[&str]) -> Release {
        Release {
            tag_name: "v1.2.3".to_owned(),
            html_url: "https://github.com/IndigoCarmine/open_draw_chemistry/releases/tag/v1.2.3"
                .to_owned(),
            assets: names.iter().map(|name| asset(name)).collect(),
        }
    }

    /// Every file `.github/workflows/release.yml` publishes for 1.2.3, signed.
    fn full_release() -> Release {
        let names = [
            "open-draw-chemistry-1.2.3-windows-x64-setup.exe",
            "open-draw-chemistry-1.2.3-windows-x64.zip",
            "open-draw-chemistry-1.2.3-macos-universal.dmg",
            "open-draw-chemistry-1.2.3-linux-x86_64.AppImage",
            "open-draw-chemistry_1.2.3_amd64.deb",
        ];
        let signatures: Vec<String> = names.iter().map(|name| format!("{name}.sig")).collect();
        let mut all: Vec<&str> = names.to_vec();
        all.extend(signatures.iter().map(String::as_str));
        release(&all)
    }

    fn version() -> Version {
        Version::new(1, 2, 3)
    }

    fn windows() -> InstallFormat {
        InstallFormat::WindowsInstaller {
            scope: Scope::CurrentUser,
            exe: PathBuf::from("app.exe"),
        }
    }

    fn formats() -> [(InstallFormat, &'static str); 3] {
        [
            (windows(), "open-draw-chemistry-1.2.3-windows-x64-setup.exe"),
            (
                InstallFormat::AppImage(PathBuf::from("/home/u/a.AppImage")),
                "open-draw-chemistry-1.2.3-linux-x86_64.AppImage",
            ),
            (
                InstallFormat::MacApp(PathBuf::from("/Applications/A.app")),
                "open-draw-chemistry-1.2.3-macos-universal.dmg",
            ),
        ]
    }

    #[test]
    fn versions_compare_as_semver() {
        let newer = |current: &str, tag: &str| {
            let release = Release {
                tag_name: tag.to_owned(),
                ..release(&[])
            };
            newer_version(current, &release)
        };
        assert_eq!(newer("0.3.0", "v0.4.0"), Some(Version::new(0, 4, 0)));
        assert_eq!(newer("0.3.0", "0.4.0"), Some(Version::new(0, 4, 0)));
        // Compared as numbers: 0.10 is after 0.9.
        assert_eq!(newer("0.9.0", "v0.10.0"), Some(Version::new(0, 10, 0)));
        assert_eq!(newer("0.3.0", "v0.3.0"), None);
        assert_eq!(newer("0.4.0", "v0.3.0"), None);
    }

    #[test]
    fn a_dev_build_sorts_before_its_release() {
        let dev = Version::parse("0.3.0-dev").unwrap();
        let release = Version::new(0, 3, 0);
        assert!(is_newer(&dev, &release));
        assert!(!is_newer(&release, &dev));
        assert!(!is_newer(&dev, &dev));
    }

    /// Build metadata does not order versions, and the build of a version is that version.
    #[test]
    fn build_metadata_does_not_make_a_version_newer() {
        let plain = Version::new(1, 2, 3);
        let built = Version::parse("1.2.3+x").unwrap();
        assert!(!is_newer(&plain, &built));
        assert!(!is_newer(&built, &plain));
        assert!(is_newer(&plain, &Version::parse("1.2.4+x").unwrap()));
        let release = Release {
            tag_name: "v1.2.3+x".to_owned(),
            ..release(&[])
        };
        assert_eq!(newer_version("1.2.3", &release), None);
    }

    #[test]
    fn a_dev_build_is_never_updated() {
        assert!(!is_updatable("0.3.0-dev"));
        assert!(!is_updatable("0.0.0-dev"));
        assert!(!is_updatable("not a version"));
        // A tag release.yml lets through but semver does not read.
        assert!(!is_updatable("01.2.3"));
        assert!(is_updatable("0.3.0"));
        // Even by a release that semver puts after it.
        let release = release(&[]);
        assert_eq!(newer_version("1.2.3-dev", &release), None);
        assert_eq!(newer_version("1.0.0-dev", &release), None);
    }

    #[test]
    fn a_pre_release_or_unreadable_tag_is_not_offered() {
        for tag in ["v9.0.0-rc.1", "v01.2.3", "nightly", ""] {
            let release = Release {
                tag_name: tag.to_owned(),
                ..release(&[])
            };
            assert_eq!(newer_version("0.3.0", &release), None, "{tag}");
        }
    }

    #[test]
    fn the_asset_for_each_format_is_chosen() {
        for (format, name) in formats() {
            let Plan::Install(download) =
                plan(format.clone(), &full_release(), &version(), Some(key()))
            else {
                panic!("{name}: not installed automatically");
            };
            assert_eq!(download.format, format);
            assert_eq!(download.asset.name, name);
            assert_eq!(download.signature.name, format!("{name}.sig"));
            assert_eq!(download.key, key());
        }
    }

    #[test]
    fn unsupported_formats_go_to_the_release_page() {
        assert_eq!(
            plan(
                InstallFormat::Other,
                &full_release(),
                &version(),
                Some(key())
            ),
            Plan::ReleasePage(Why::UnsupportedFormat)
        );
    }

    #[test]
    fn the_placeholder_key_goes_to_the_release_page() {
        for (format, _) in formats() {
            assert_eq!(
                plan(format, &full_release(), &version(), None),
                Plan::ReleasePage(Why::NoSigningKey)
            );
        }
    }

    #[test]
    fn a_missing_asset_or_signature_goes_to_the_release_page() {
        let (format, name) = formats().into_iter().next().unwrap();
        let unsigned = release(&[name]);
        assert_eq!(
            plan(format.clone(), &unsigned, &version(), Some(key())),
            Plan::ReleasePage(Why::NoAsset)
        );
        let signature = format!("{name}.sig");
        let no_asset = release(&[&signature]);
        assert_eq!(
            plan(format, &no_asset, &version(), Some(key())),
            Plan::ReleasePage(Why::NoAsset)
        );
    }

    /// The asset of another version is not picked up, whatever the release lists.
    #[test]
    fn an_asset_of_another_version_is_not_used() {
        let old = "open-draw-chemistry-1.2.2-windows-x64-setup.exe";
        let signature = format!("{old}.sig");
        let release = release(&[old, &signature]);
        assert_eq!(
            plan(windows(), &release, &version(), Some(key())),
            Plan::ReleasePage(Why::NoAsset)
        );
    }

    #[test]
    fn an_asset_hosted_elsewhere_goes_to_the_release_page() {
        for url in [
            "https://example.com/x",
            "http://github.com/IndigoCarmine/open_draw_chemistry/releases/download/v1.2.3/x",
            "https://github.com/someone/else/releases/download/v1.2.3/x",
        ] {
            let mut release = full_release();
            release.assets[0].browser_download_url = url.to_owned();
            assert_eq!(
                plan(windows(), &release, &version(), Some(key())),
                Plan::ReleasePage(Why::NoAsset),
                "{url}"
            );
        }
    }

    #[test]
    fn the_release_page_is_this_repositorys_or_the_latest() {
        let tag = "https://github.com/IndigoCarmine/open_draw_chemistry/releases/tag/v1.2.3";
        assert_eq!(release(&[]).page(), tag);
        for url in [
            "",
            "https://example.com/IndigoCarmine/open_draw_chemistry/releases/tag/v1",
            "http://github.com/IndigoCarmine/open_draw_chemistry/releases/tag/v1",
            "https://github.com/someone/else/releases/tag/v1",
            "https://github.com/IndigoCarmine/open_draw_chemistry/releasesevil",
        ] {
            let release = Release {
                html_url: url.to_owned(),
                ..release(&[])
            };
            assert_eq!(release.page(), LATEST_RELEASE_PAGE, "{url}");
        }
    }

    /// The page goes into a cmd.exe script (see `install::SETUP_SCRIPT`), inside double quotes.
    #[test]
    fn the_release_page_cannot_break_out_of_quotes() {
        let release = Release {
            html_url: concat!(
                "https://github.com/IndigoCarmine/open_draw_chemistry/releases/tag/v1",
                "\"&calc&\"?a=\"b\"#\"c\""
            )
            .to_owned(),
            ..release(&[])
        };
        assert!(!release.page().contains('"'), "{}", release.page());
    }

    #[test]
    fn githubs_answer_is_read() {
        let json = br#"{
            "tag_name": "v1.2.3",
            "html_url": "https://github.com/IndigoCarmine/open_draw_chemistry/releases/tag/v1.2.3",
            "draft": false,
            "prerelease": false,
            "assets": [{
                "name": "a.exe",
                "size": 12,
                "browser_download_url": "https://github.com/a.exe",
                "content_type": "application/octet-stream"
            }]
        }"#;
        let release = Release::parse(json).unwrap();
        assert_eq!(release.tag_name, "v1.2.3");
        assert_eq!(release.assets[0].size, 12);
        assert!(matches!(
            Release::parse(b"<html>"),
            Err(Error::Malformed(_))
        ));
    }
}
