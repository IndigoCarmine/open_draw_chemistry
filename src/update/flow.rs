//! Where an update has got to, from the check at startup to the restart. The UI (see
//! [`crate::ui::update`]) only draws this and passes on the user's answers; the threads here
//! cannot wake the UI, so it keeps frames coming while [`UpdateFlow::is_busy`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use semver::Version;

use super::http::Transport;
use super::install::{self, InstallFormat, Prepared};
use super::release::{Download, Plan};
use super::signature::PublicKey;
use super::{Available, Error};

/// What the app must do next.
#[derive(Debug)]
pub enum Event {
    /// Open this Release page.
    OpenPage(String),
    /// The automatic update failed: say so and open this Release page.
    Failed { error: Error, release_page: String },
    /// A verified update is ready: quit, so that it can be installed.
    Quit,
}

struct Downloading {
    version: Version,
    release_page: String,
    total: u64,
    progress: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    result: Receiver<Result<Prepared, Error>>,
}

#[derive(Default)]
enum State {
    #[default]
    Idle,
    Checking {
        transport: Arc<dyn Transport>,
        result: Receiver<Option<Available>>,
    },
    Prompt {
        transport: Arc<dyn Transport>,
        available: Available,
    },
    Downloading(Downloading),
    Ready {
        prepared: Prepared,
        release_page: String,
        /// The app is quitting to install it, rather than the user having called that off.
        quitting: bool,
    },
}

/// What there is to show.
#[derive(Debug, PartialEq, Eq)]
pub enum View<'a> {
    Nothing,
    Prompt(&'a Available),
    Downloading {
        version: &'a Version,
        done: u64,
        total: u64,
    },
    /// Downloaded and verified, and the user has not said to restart yet.
    Ready,
}

/// The update in progress, if any. This lasts one run of the app: a No or a failure is only
/// asked about again at the next startup.
#[derive(Default)]
pub struct UpdateFlow {
    state: State,
}

impl UpdateFlow {
    /// Start the one check of this run in the background. Any failure of it stays silent.
    pub fn start_check(
        &mut self,
        transport: Arc<dyn Transport>,
        current: &'static str,
        format: impl FnOnce() -> InstallFormat + Send + 'static,
        key: Option<PublicKey>,
    ) {
        let (send, result) = mpsc::channel();
        let thread_transport = transport.clone();
        std::thread::spawn(move || {
            install::remove_stale(&std::env::temp_dir());
            let available = super::check(thread_transport.as_ref(), current, format(), key);
            let _ = send.send(available.ok().flatten());
        });
        self.state = State::Checking { transport, result };
    }

    pub fn is_busy(&self) -> bool {
        matches!(self.state, State::Checking { .. } | State::Downloading(_))
    }

    pub fn view(&self) -> View<'_> {
        match &self.state {
            State::Prompt { available, .. } => View::Prompt(available),
            State::Downloading(download) => View::Downloading {
                version: &download.version,
                done: download.progress.load(Ordering::Relaxed),
                total: download.total,
            },
            State::Ready {
                quitting: false, ..
            } => View::Ready,
            _ => View::Nothing,
        }
    }

    /// Collect what the background threads have finished.
    pub fn poll(&mut self) -> Option<Event> {
        match &mut self.state {
            State::Checking { result, .. } => match result.try_recv() {
                Err(TryRecvError::Empty) => None,
                Ok(Some(available)) => {
                    if let State::Checking { transport, .. } = std::mem::take(&mut self.state) {
                        self.state = State::Prompt {
                            transport,
                            available,
                        };
                    }
                    None
                }
                Ok(None) | Err(TryRecvError::Disconnected) => {
                    self.state = State::Idle;
                    None
                }
            },
            State::Downloading(download) => {
                let result = match download.result.try_recv() {
                    Err(TryRecvError::Empty) => return None,
                    Ok(result) => result,
                    Err(TryRecvError::Disconnected) => {
                        Err(Error::Install("the download stopped".to_owned()))
                    }
                };
                let release_page = std::mem::take(&mut download.release_page);
                self.state = State::Idle;
                match result {
                    Ok(prepared) => {
                        self.state = State::Ready {
                            prepared,
                            release_page,
                            quitting: true,
                        };
                        Some(Event::Quit)
                    }
                    Err(Error::Cancelled) => None,
                    Err(error) => Some(Event::Failed {
                        error,
                        release_page,
                    }),
                }
            }
            _ => None,
        }
    }

    /// The user's answer to the prompt.
    pub fn answer(&mut self, yes: bool) -> Option<Event> {
        if !matches!(self.state, State::Prompt { .. }) {
            return None;
        }
        let State::Prompt {
            transport,
            available,
        } = std::mem::take(&mut self.state)
        else {
            return None;
        };
        if !yes {
            return None;
        }
        match available.plan {
            Plan::ReleasePage(_) => Some(Event::OpenPage(available.release_page)),
            Plan::Install(download) => {
                self.start_download(
                    transport,
                    download,
                    available.version,
                    available.release_page,
                );
                None
            }
        }
    }

    fn start_download(
        &mut self,
        transport: Arc<dyn Transport>,
        download: Download,
        version: Version,
        release_page: String,
    ) {
        let progress = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (send, result) = mpsc::channel();
        let total = download.asset.size;
        let (thread_progress, thread_cancel) = (progress.clone(), cancel.clone());
        std::thread::spawn(move || {
            let prepared = install::prepare(
                transport.as_ref(),
                &download,
                &thread_cancel,
                &thread_progress,
            );
            let _ = send.send(prepared);
        });
        self.state = State::Downloading(Downloading {
            version,
            release_page,
            total,
            progress,
            cancel,
            result,
        });
    }

    /// Stop a download at once. Its thread cleans up after itself; there is nothing to open.
    pub fn cancel(&mut self) {
        if let State::Downloading(download) = &self.state {
            download.cancel.store(true, Ordering::Relaxed);
            self.state = State::Idle;
        }
    }

    /// The user asked to restart into the update.
    pub fn restart(&mut self) -> Option<Event> {
        match &mut self.state {
            State::Ready { quitting, .. } => {
                *quitting = true;
                Some(Event::Quit)
            }
            _ => None,
        }
    }

    /// The app is not quitting after all (the user cancelled saving their documents): ask again
    /// rather than install at some later, unrelated quit.
    pub fn quit_called_off(&mut self) {
        if let State::Ready { quitting, .. } = &mut self.state {
            *quitting = false;
        }
    }

    /// Whether the app is meant to be quitting to install the update.
    pub fn is_quitting(&self) -> bool {
        matches!(self.state, State::Ready { quitting: true, .. })
    }

    /// The user does not want the update after all: delete it.
    pub fn discard(&mut self) {
        if matches!(self.state, State::Ready { .. }) {
            self.state = State::Idle;
        }
    }

    /// As the app exits: the update to install now, with its Release page.
    pub fn take_for_exit(&mut self) -> Option<(Prepared, String)> {
        if !self.is_quitting() {
            return None;
        }
        match std::mem::take(&mut self.state) {
            State::Ready {
                prepared,
                release_page,
                ..
            } => Some((prepared, release_page)),
            _ => None,
        }
    }
}

/// Everything in this module's check and download, on a fake network.
#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ring::signature::{Ed25519KeyPair, KeyPair};

    use super::super::http::testing::{Canned, FakeTransport};
    use super::super::install::Scope;
    use super::super::signature::signed_message;
    use super::super::{LATEST_RELEASE_API, release::Why};
    use super::*;

    const NAME: &str = "open-draw-chemistry-1.2.3-windows-x64-setup.exe";
    const PAGE: &str = "https://github.com/IndigoCarmine/open_draw_chemistry/releases/tag/v1.2.3";
    const SETUP: &[u8] = b"a new installer";

    fn asset_url(name: &str) -> String {
        format!(
            "https://github.com/IndigoCarmine/open_draw_chemistry/releases/download/v1.2.3/{name}"
        )
    }

    /// A release of 1.2.3 with a signed Windows installer, and the key that signed it.
    fn network() -> (FakeTransport, PublicKey) {
        let keys = Ed25519KeyPair::from_seed_unchecked(&[5; 32]).unwrap();
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let key = PublicKey::parse(&hex(keys.public_key().as_ref())).unwrap();
        let sha256 = hex(ring::digest::digest(&ring::digest::SHA256, SETUP).as_ref());
        let signature = keys.sign(signed_message(NAME, &sha256).as_bytes());
        let json = serde_json::json!({
            "tag_name": "v1.2.3",
            "html_url": PAGE,
            "assets": [
                { "name": NAME, "size": SETUP.len(), "browser_download_url": asset_url(NAME) },
                {
                    "name": format!("{NAME}.sig"),
                    "size": 64,
                    "browser_download_url": asset_url(&format!("{NAME}.sig"))
                },
            ],
        });
        let storage = "https://objects.githubusercontent.com/blob";
        let transport = FakeTransport::default()
            .with(
                LATEST_RELEASE_API,
                Canned::Body(json.to_string().into_bytes()),
            )
            .with(&asset_url(NAME), Canned::Redirect(storage.to_owned()))
            .with(storage, Canned::Body(SETUP.to_vec()))
            .with(
                &asset_url(&format!("{NAME}.sig")),
                Canned::Body(signature.as_ref().to_vec()),
            );
        (transport, key)
    }

    fn windows() -> InstallFormat {
        InstallFormat::WindowsInstaller {
            scope: Scope::CurrentUser,
            exe: "app.exe".into(),
        }
    }

    /// Run `flow`'s threads until `done`, polling as the UI does.
    fn poll_until(flow: &mut UpdateFlow, done: impl Fn(&UpdateFlow) -> bool) -> Option<Event> {
        let start = Instant::now();
        while !done(flow) {
            assert!(start.elapsed() < Duration::from_secs(10), "timed out");
            if let Some(event) = flow.poll() {
                return Some(event);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        None
    }

    fn checked(
        transport: FakeTransport,
        current: &'static str,
        format: InstallFormat,
        key: Option<PublicKey>,
    ) -> UpdateFlow {
        let mut flow = UpdateFlow::default();
        flow.start_check(Arc::new(transport), current, move || format, key);
        poll_until(&mut flow, |flow| !flow.is_busy());
        flow
    }

    #[test]
    fn a_newer_release_prompts() {
        let (transport, key) = network();
        let flow = checked(transport, "1.0.0", windows(), Some(key));
        let View::Prompt(available) = flow.view() else {
            panic!("no prompt");
        };
        assert_eq!(available.version, Version::new(1, 2, 3));
        assert_eq!(available.release_page, PAGE);
        assert!(matches!(available.plan, Plan::Install(_)));
    }

    #[test]
    fn nothing_is_shown_without_a_newer_release() {
        for current in ["1.2.3", "2.0.0", "1.0.0-dev"] {
            let (transport, key) = network();
            let flow = checked(transport, current, windows(), Some(key));
            assert_eq!(flow.view(), View::Nothing, "{current}");
        }
    }

    #[test]
    fn a_dev_build_does_not_ask_github() {
        let (transport, key) = network();
        let transport = Arc::new(transport);
        let mut flow = UpdateFlow::default();
        flow.start_check(transport.clone(), "1.0.0-dev", windows, Some(key));
        poll_until(&mut flow, |flow| !flow.is_busy());
        assert!(transport.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn a_failed_check_stays_silent() {
        for transport in [
            FakeTransport::default(),
            FakeTransport::default().with(LATEST_RELEASE_API, Canned::Status(403)),
            FakeTransport::default().with(LATEST_RELEASE_API, Canned::Body(b"<html>".to_vec())),
        ] {
            let flow = checked(transport, "1.0.0", windows(), None);
            assert_eq!(flow.view(), View::Nothing);
        }
    }

    #[test]
    fn no_does_nothing() {
        let (transport, key) = network();
        let mut flow = checked(transport, "1.0.0", windows(), Some(key));
        assert!(flow.answer(false).is_none());
        assert_eq!(flow.view(), View::Nothing);
        assert!(!flow.is_busy());
    }

    #[test]
    fn yes_with_the_placeholder_key_opens_the_release_page() {
        let (transport, _) = network();
        let mut flow = checked(transport, "1.0.0", windows(), None);
        let View::Prompt(available) = flow.view() else {
            panic!("no prompt");
        };
        assert_eq!(available.plan, Plan::ReleasePage(Why::NoSigningKey));
        match flow.answer(true) {
            Some(Event::OpenPage(page)) => assert_eq!(page, PAGE),
            other => panic!("{other:?}"),
        }
        assert_eq!(flow.view(), View::Nothing);
    }

    #[test]
    fn yes_on_an_unsupported_format_opens_the_release_page() {
        let (transport, key) = network();
        let mut flow = checked(transport, "1.0.0", InstallFormat::Other, Some(key));
        assert!(matches!(flow.answer(true), Some(Event::OpenPage(_))));
    }

    #[test]
    fn yes_downloads_verifies_and_quits() {
        let (transport, key) = network();
        let mut flow = checked(transport, "1.0.0", windows(), Some(key));
        assert!(flow.answer(true).is_none());
        assert!(flow.is_busy());
        let event = poll_until(&mut flow, |_| false);
        assert!(matches!(event, Some(Event::Quit)), "{event:?}");
        let (prepared, page) = flow.take_for_exit().expect("an update to install");
        assert_eq!(page, PAGE);
        let Prepared::WindowsInstaller { setup, .. } = prepared else {
            panic!("not an installer");
        };
        assert_eq!(std::fs::read(setup).unwrap(), SETUP);
    }

    #[test]
    fn a_tampered_download_falls_back_to_the_release_page() {
        let (mut transport, key) = network();
        transport.replies.insert(
            "https://objects.githubusercontent.com/blob".to_owned(),
            Canned::Body(b"an old installer".to_vec()[..SETUP.len()].to_vec()),
        );
        let mut flow = checked(transport, "1.0.0", windows(), Some(key));
        flow.answer(true);
        match poll_until(&mut flow, |_| false) {
            Some(Event::Failed {
                error: Error::Signature,
                release_page,
            }) => assert_eq!(release_page, PAGE),
            other => panic!("{other:?}"),
        }
        assert!(flow.take_for_exit().is_none());
    }

    #[test]
    fn a_failed_download_falls_back_to_the_release_page() {
        let (mut transport, key) = network();
        transport.replies.insert(
            "https://objects.githubusercontent.com/blob".to_owned(),
            Canned::Status(500),
        );
        let mut flow = checked(transport, "1.0.0", windows(), Some(key));
        flow.answer(true);
        assert!(matches!(
            poll_until(&mut flow, |_| false),
            Some(Event::Failed { .. })
        ));
    }

    #[test]
    fn a_cancelled_download_ends_quietly() {
        let (transport, key) = network();
        let mut flow = checked(transport, "1.0.0", windows(), Some(key));
        flow.answer(true);
        flow.cancel();
        assert_eq!(flow.view(), View::Nothing);
        assert!(!flow.is_busy());
        assert!(flow.poll().is_none());
    }

    /// Cancelling the Save dialog must not leave an install armed for a later quit.
    #[test]
    fn a_called_off_quit_asks_again() {
        let (transport, key) = network();
        let mut flow = checked(transport, "1.0.0", windows(), Some(key));
        flow.answer(true);
        poll_until(&mut flow, |_| false);
        assert!(flow.is_quitting());
        assert_eq!(flow.view(), View::Nothing);

        flow.quit_called_off();
        assert_eq!(flow.view(), View::Ready);
        assert!(flow.take_for_exit().is_none());

        assert!(matches!(flow.restart(), Some(Event::Quit)));
        assert!(flow.take_for_exit().is_some());
    }

    #[test]
    fn a_discarded_update_is_gone() {
        let (transport, key) = network();
        let mut flow = checked(transport, "1.0.0", windows(), Some(key));
        flow.answer(true);
        poll_until(&mut flow, |_| false);
        flow.quit_called_off();
        flow.discard();
        assert_eq!(flow.view(), View::Nothing);
        assert!(flow.restart().is_none());
    }
}
