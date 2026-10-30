//! The update check at startup and the automatic install.
//!
//! A background thread asks GitHub for the latest release ([`release`]). When it is newer than
//! this build the user is asked what to do ([`flow`], drawn by [`crate::ui::update`]): Yes
//! downloads the asset for the way this copy was installed, verifies its signature
//! ([`signature`]) and installs it ([`install`]). Whatever cannot be done automatically ends at
//! the Release page in the browser. An update check that fails, offline say, stays silent.
//!
//! All network traffic goes through [`http::Transport`], so the tests never need the network.

use std::fmt;

use semver::Version;

pub mod flow;
pub mod http;
pub mod install;
pub mod release;
pub mod signature;

use http::Transport;
use install::InstallFormat;
use release::{Plan, Release};
use signature::PublicKey;

/// The latest release, as GitHub's API describes it.
pub const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/IndigoCarmine/open_draw_chemistry/releases/latest";

/// The Release page used when GitHub's answer does not name a usable one.
pub const LATEST_RELEASE_PAGE: &str =
    "https://github.com/IndigoCarmine/open_draw_chemistry/releases/latest";

/// More than GitHub's description of a release with a long changelog ever takes.
const API_ANSWER_LIMIT: u64 = 4 * 1024 * 1024;

#[derive(Debug)]
pub enum Error {
    /// A URL that is not one of GitHub's over HTTPS (see [`http::is_allowed`]).
    Refused(String),
    Network(String),
    Status(u16),
    /// The download is not the size the release says it is.
    Size,
    Malformed(String),
    Signature,
    Cancelled,
    Io(std::io::Error),
    Install(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Refused(url) => f.write_str(&t!("update.error.refused", url = url)),
            Error::Network(error) => f.write_str(&t!("update.error.network", error = error)),
            Error::Status(status) => f.write_str(&t!("update.error.status", status = status)),
            Error::Size => f.write_str(t!("update.error.size")),
            Error::Malformed(error) => f.write_str(&t!("update.error.malformed", error = error)),
            Error::Signature => f.write_str(t!("update.error.signature")),
            Error::Cancelled => f.write_str(t!("update.error.cancelled")),
            Error::Io(e) => write!(f, "{e}"),
            Error::Install(error) => f.write_str(&t!("update.error.install", error = error)),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

/// A newer release, and what answering Yes to it will do.
#[derive(Debug, PartialEq, Eq)]
pub struct Available {
    pub version: Version,
    pub release_page: String,
    pub plan: Plan,
}

/// Ask GitHub whether there is a release newer than `current`. A pre-release `current` (every
/// local `-dev` build) never asks.
pub fn check(
    transport: &dyn Transport,
    current: &str,
    format: InstallFormat,
    key: Option<PublicKey>,
) -> Result<Option<Available>, Error> {
    if !release::is_updatable(current) {
        return Ok(None);
    }
    let answer = http::read_limited(transport, LATEST_RELEASE_API, API_ANSWER_LIMIT)?;
    let release = Release::parse(&answer)?;
    Ok(
        release::newer_version(current, &release).map(|version| Available {
            plan: release::plan(format, &release, &version, key),
            release_page: release.page(),
            version,
        }),
    )
}

/// Show `url`, which has been through [`release::Release::page`], in the browser.
pub fn open_page(url: &str) {
    // Nothing more can be done when no browser opens.
    let _ = webbrowser::open(url);
}
