//! HTTPS, restricted to GitHub.
//!
//! [`Transport`] makes a single request and does not follow redirects, so that [`open`] can vet
//! every hop against [`is_allowed`]: a download starts at github.com and is redirected to its
//! storage hosts. The tests replace the real one, from [`system`], with a fake.

use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(any(windows, target_os = "macos"))]
use std::time::Duration;

use url::Url;

use super::Error;

/// Where GitHub serves its API, its Release pages and its release assets.
const ALLOWED_HOSTS: [&str; 4] = [
    "api.github.com",
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

/// Hops [`open`] follows; a release asset takes one.
const MAX_REDIRECTS: usize = 5;

/// The answer to one request.
pub enum Reply {
    Body(Box<dyn Read + Send>),
    /// A redirect, to this `Location`.
    Redirect(String),
}

/// One HTTP GET. A status other than success or a redirect is an error.
pub trait Transport: Send + Sync {
    fn get(&self, url: &str) -> Result<Reply, Error>;
}

/// Whether `url` may be requested: HTTPS to one of GitHub's hosts, with nothing that could make
/// it mean another (`https://github.com@example.com/`, a port).
pub fn is_allowed(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url
            .host_str()
            .is_some_and(|host| ALLOWED_HOSTS.contains(&host))
}

/// `text` as a URL, when it may be requested.
fn vetted(text: &str) -> Result<Url, Error> {
    Url::parse(text)
        .ok()
        .filter(is_allowed)
        .ok_or_else(|| Error::Refused(text.to_owned()))
}

/// GET `url`, following redirects to allowed URLs only.
pub fn open(transport: &dyn Transport, url: &str) -> Result<Box<dyn Read + Send>, Error> {
    let mut url = vetted(url)?;
    for _ in 0..=MAX_REDIRECTS {
        match transport.get(url.as_str())? {
            Reply::Body(body) => return Ok(body),
            Reply::Redirect(location) => {
                url = url
                    .join(&location)
                    .ok()
                    .filter(is_allowed)
                    .ok_or(Error::Refused(location))?;
            }
        }
    }
    Err(Error::Network("too many redirects".to_owned()))
}

/// The whole body of `url`, which must not be longer than `limit`.
pub fn read_limited(transport: &dyn Transport, url: &str, limit: u64) -> Result<Vec<u8>, Error> {
    let mut body = Vec::new();
    open(transport, url)?
        .take(limit + 1)
        .read_to_end(&mut body)
        .map_err(|e| Error::Network(e.to_string()))?;
    if body.len() as u64 > limit {
        return Err(Error::Size);
    }
    Ok(body)
}

/// Write the body of `url`, which must be exactly `size` bytes, to `out`. `progress` follows the
/// bytes written and `cancel` stops it.
pub fn download(
    transport: &dyn Transport,
    url: &str,
    size: u64,
    out: &mut dyn Write,
    cancel: &AtomicBool,
    progress: &AtomicU64,
) -> Result<(), Error> {
    let mut body = open(transport, url)?;
    let mut buffer = vec![0; 64 * 1024];
    let mut written = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let read = match body.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Error::Network(e.to_string())),
        };
        written += read as u64;
        if written > size {
            return Err(Error::Size);
        }
        out.write_all(&buffer[..read])?;
        progress.store(written, Ordering::Relaxed);
    }
    if written == size {
        Ok(())
    } else {
        Err(Error::Size)
    }
}

/// The real network, through what this OS provides for TLS.
pub fn system() -> Arc<dyn Transport> {
    #[cfg(any(windows, target_os = "macos"))]
    return Arc::new(Ureq::new());
    #[cfg(not(any(windows, target_os = "macos")))]
    return Arc::new(super::curl::Curl::new());
}

/// HTTPS over native-tls.
#[cfg(any(windows, target_os = "macos"))]
struct Ureq(ureq::Agent);

#[cfg(any(windows, target_os = "macos"))]
impl Ureq {
    fn new() -> Self {
        use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

        let config = ureq::Agent::config_builder()
            .tls_config(
                TlsConfig::builder()
                    .provider(TlsProvider::NativeTls)
                    .root_certs(RootCerts::PlatformVerifier)
                    .build(),
            )
            .https_only(true)
            .max_redirects(0)
            .http_status_as_error(false)
            .user_agent(format!("open-draw-chemistry/{}", crate::APP_VERSION))
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            // A cancelled download only stops when a read returns, so a stalled one must end.
            .timeout_recv_body(Some(Duration::from_secs(30 * 60)))
            .build();
        Self(config.new_agent())
    }
}

#[cfg(any(windows, target_os = "macos"))]
impl Transport for Ureq {
    fn get(&self, url: &str) -> Result<Reply, Error> {
        let response = self
            .0
            .get(url)
            .call()
            .map_err(|e| Error::Network(e.to_string()))?;
        let status = response.status();
        if status.is_redirection() {
            return response
                .headers()
                .get(ureq::http::header::LOCATION)
                .and_then(|location| location.to_str().ok())
                .map(|location| Reply::Redirect(location.to_owned()))
                .ok_or(Error::Status(status.as_u16()));
        }
        if !status.is_success() {
            return Err(Error::Status(status.as_u16()));
        }
        Ok(Reply::Body(Box::new(response.into_body().into_reader())))
    }
}

#[cfg(test)]
pub mod testing {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    /// A canned answer.
    pub enum Canned {
        Body(Vec<u8>),
        Redirect(String),
        Status(u16),
    }

    /// Answers from a table, and remembers what was asked for.
    #[derive(Default)]
    pub struct FakeTransport {
        pub replies: HashMap<String, Canned>,
        pub requests: Mutex<Vec<String>>,
    }

    impl FakeTransport {
        pub fn with(mut self, url: &str, reply: Canned) -> Self {
            self.replies.insert(url.to_owned(), reply);
            self
        }
    }

    impl Transport for FakeTransport {
        fn get(&self, url: &str) -> Result<Reply, Error> {
            self.requests.lock().unwrap().push(url.to_owned());
            match self.replies.get(url) {
                Some(Canned::Body(bytes)) => {
                    Ok(Reply::Body(Box::new(std::io::Cursor::new(bytes.clone()))))
                }
                Some(Canned::Redirect(to)) => Ok(Reply::Redirect(to.clone())),
                Some(Canned::Status(status)) => Err(Error::Status(*status)),
                None => Err(Error::Network(format!("no canned reply for {url}"))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{Canned, FakeTransport};
    use super::*;

    /// The real thing, by hand (`cargo test real_github -- --ignored`): the API, and an asset
    /// through its redirect to GitHub's storage, which the allowed hosts must cover.
    #[test]
    #[ignore = "needs the network"]
    fn real_github_serves_the_latest_release_and_its_assets() {
        let transport = system();
        let transport = transport.as_ref();
        let answer = read_limited(transport, crate::update::LATEST_RELEASE_API, 4 << 20).unwrap();
        let release = crate::update::release::Release::parse(&answer).unwrap();
        let asset = release
            .assets
            .iter()
            .min_by_key(|asset| asset.size)
            .unwrap();
        let mut out = Vec::new();
        download(
            transport,
            &asset.browser_download_url,
            asset.size,
            &mut out,
            &AtomicBool::new(false),
            &AtomicU64::new(0),
        )
        .unwrap();
        assert_eq!(out.len() as u64, asset.size);
    }

    fn allowed(url: &str) -> bool {
        is_allowed(&Url::parse(url).unwrap())
    }

    #[test]
    fn only_githubs_https_urls_are_allowed() {
        assert!(allowed("https://api.github.com/repos/a/b/releases/latest"));
        assert!(allowed("https://github.com/a/b/releases/download/v1/x.exe"));
        assert!(allowed("https://GitHub.com/a"));
        assert!(allowed("https://objects.githubusercontent.com/x"));
        assert!(allowed("https://release-assets.githubusercontent.com/x"));

        assert!(!allowed("http://github.com/a"));
        assert!(!allowed("ftp://github.com/a"));
        assert!(!allowed("https://example.com/a"));
        assert!(!allowed("https://github.com.example.com/a"));
        assert!(!allowed("https://notgithub.com/a"));
        assert!(!allowed("https://evil.objects.githubusercontent.com/a"));
        assert!(!allowed("https://github.com@example.com/a"));
        assert!(!allowed("https://user@github.com/a"));
        assert!(!allowed("https://github.com:8443/a"));
    }

    #[test]
    fn redirects_are_followed_between_allowed_hosts() {
        let start = "https://github.com/a/x.exe";
        let storage = "https://objects.githubusercontent.com/blob";
        let transport = FakeTransport::default()
            .with(start, Canned::Redirect(storage.to_owned()))
            .with(storage, Canned::Body(b"bytes".to_vec()));
        assert_eq!(read_limited(&transport, start, 100).unwrap(), b"bytes");
    }

    #[test]
    fn a_relative_redirect_stays_on_the_host() {
        let start = "https://github.com/a/x.exe";
        let next = "https://github.com/b/x.exe";
        let transport = FakeTransport::default()
            .with(start, Canned::Redirect("/b/x.exe".to_owned()))
            .with(next, Canned::Body(b"bytes".to_vec()));
        assert_eq!(read_limited(&transport, start, 100).unwrap(), b"bytes");
    }

    #[test]
    fn a_redirect_off_github_is_refused_before_it_is_requested() {
        for location in [
            "https://example.com/x.exe",
            "http://objects.githubusercontent.com/x.exe",
        ] {
            let start = "https://github.com/a/x.exe";
            let transport = FakeTransport::default()
                .with(start, Canned::Redirect(location.to_owned()))
                .with(location, Canned::Body(b"evil".to_vec()));
            assert!(matches!(
                read_limited(&transport, start, 100),
                Err(Error::Refused(_))
            ));
            assert_eq!(*transport.requests.lock().unwrap(), [start]);
        }
    }

    #[test]
    fn a_url_off_github_is_refused_before_it_is_requested() {
        let transport = FakeTransport::default();
        assert!(matches!(
            read_limited(&transport, "https://example.com/x", 100),
            Err(Error::Refused(_))
        ));
        assert!(transport.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn a_redirect_loop_ends() {
        let start = "https://github.com/a";
        let transport = FakeTransport::default().with(start, Canned::Redirect(start.to_owned()));
        assert!(matches!(
            read_limited(&transport, start, 100),
            Err(Error::Network(_))
        ));
    }

    #[test]
    fn a_body_over_the_limit_is_refused() {
        let url = "https://github.com/a";
        let transport = FakeTransport::default().with(url, Canned::Body(vec![0; 101]));
        assert!(matches!(
            read_limited(&transport, url, 100),
            Err(Error::Size)
        ));
        assert_eq!(read_limited(&transport, url, 101).unwrap().len(), 101);
    }

    fn download_to_vec(transport: &FakeTransport, size: u64) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        let progress = AtomicU64::new(0);
        download(
            transport,
            "https://github.com/a",
            size,
            &mut out,
            &AtomicBool::new(false),
            &progress,
        )?;
        assert_eq!(progress.load(Ordering::Relaxed), out.len() as u64);
        Ok(out)
    }

    #[test]
    fn a_download_must_be_the_expected_size() {
        let transport =
            FakeTransport::default().with("https://github.com/a", Canned::Body(vec![7; 100_000]));
        assert_eq!(download_to_vec(&transport, 100_000).unwrap().len(), 100_000);
        assert!(matches!(
            download_to_vec(&transport, 99_999),
            Err(Error::Size)
        ));
        assert!(matches!(
            download_to_vec(&transport, 100_001),
            Err(Error::Size)
        ));
    }

    #[test]
    fn a_cancelled_download_stops() {
        let transport =
            FakeTransport::default().with("https://github.com/a", Canned::Body(vec![7; 10]));
        let mut out = Vec::new();
        let result = download(
            &transport,
            "https://github.com/a",
            10,
            &mut out,
            &AtomicBool::new(true),
            &AtomicU64::new(0),
        );
        assert!(matches!(result, Err(Error::Cancelled)));
        assert!(out.is_empty());
    }

    #[test]
    fn a_failed_status_is_an_error() {
        let url = "https://github.com/a";
        let transport = FakeTransport::default().with(url, Canned::Status(404));
        assert!(matches!(
            read_limited(&transport, url, 100),
            Err(Error::Status(404))
        ));
    }
}
