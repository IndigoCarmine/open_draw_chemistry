//! HTTPS through the system's `curl` program on Linux, so the app links no TLS library there
//! (see "HTTPS" in docs/DEVELOPMENT.md). Without `curl` every request fails, so the update check
//! stays silent, as it does offline.
//!
//! curl makes one request and follows no redirect: the status and `Location` come back ahead of
//! the body on its output, and [`super::http::open`] decides about the next hop as it does for
//! any [`Transport`]. Bytes arrive as curl receives them, so a download's progress needs nothing
//! from curl's own progress display.

use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Read};
use std::process::{Child, ChildStdout, Command, Stdio};

use super::Error;
use super::http::{Reply, Transport};

/// Far more than GitHub's answer ever has ahead of its body.
const HEAD_LIMIT: u64 = 64 * 1024;

pub struct Curl {
    program: OsString,
}

impl Curl {
    pub fn new() -> Self {
        Self::with_program("curl")
    }

    fn with_program(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
        }
    }
}

/// The command line for one GET of `url`.
fn arguments(url: &str) -> Vec<String> {
    let user_agent = format!("open-draw-chemistry/{}", crate::APP_VERSION);
    [
        // Only counts as the first argument. A ~/.curlrc could follow redirects or trust anything.
        "--disable",
        "--silent",
        "--show-error",
        "--include",
        "--suppress-connect-headers",
        "--proto",
        "=https",
        "--tlsv1.2",
        "--globoff",
        "--connect-timeout",
        "15",
        // A cancelled download only stops when a read returns, so a stalled one must end.
        "--speed-limit",
        "1",
        "--speed-time",
        "30",
        "--max-time",
        "1800",
        "--user-agent",
        &user_agent,
        "--url",
        url,
    ]
    .map(str::to_owned)
    .to_vec()
}

/// The part of a response's head that matters here.
#[derive(Debug, PartialEq, Eq)]
struct Head {
    status: u16,
    location: Option<String>,
}

/// Read the final head from `output`, leaving the body to be read. `None` when there is none:
/// curl failed first, or this is not HTTP.
fn read_head(output: &mut impl BufRead) -> Option<Head> {
    let mut output = output.take(HEAD_LIMIT);
    let mut line = Vec::new();
    loop {
        line.clear();
        output.read_until(b'\n', &mut line).ok()?;
        let status_line = String::from_utf8_lossy(&line);
        if !status_line.starts_with("HTTP/") {
            return None;
        }
        let mut head = Head {
            status: status_line.split_whitespace().nth(1)?.parse().ok()?,
            location: None,
        };
        loop {
            line.clear();
            if output.read_until(b'\n', &mut line).ok()? == 0 {
                return None;
            }
            let header = String::from_utf8_lossy(&line);
            let header = header.trim();
            if header.is_empty() {
                break;
            }
            if let Some((name, value)) = header.split_once(':')
                && name.eq_ignore_ascii_case("location")
            {
                head.location = Some(value.trim().to_owned());
            }
        }
        // An interim answer (103 Early Hints) is followed by the real one.
        if !(100..200).contains(&head.status) {
            return Some(head);
        }
    }
}

/// The rest of curl's output. Dropping it stops curl.
struct Body {
    output: BufReader<ChildStdout>,
    curl: Child,
    finished: bool,
}

impl Body {
    /// Why curl ended, once its output has: only an exit status tells a cut-off body from a
    /// whole one.
    fn failure(&mut self) -> Option<String> {
        self.finished = true;
        let status = match self.curl.wait() {
            Ok(status) if status.success() => return None,
            Ok(status) => status,
            Err(e) => return Some(e.to_string()),
        };
        let mut message = Vec::new();
        if let Some(stderr) = self.curl.stderr.take() {
            // What curl had to say is lost if this fails, and the status still reports it.
            let _ = stderr.take(HEAD_LIMIT).read_to_end(&mut message);
        }
        let message = String::from_utf8_lossy(&message);
        Some(match message.trim() {
            "" => format!("curl: {status}"),
            message => message.to_owned(),
        })
    }
}

impl Read for Body {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.output.read(buffer)?;
        if read == 0
            && !self.finished
            && let Some(failure) = self.failure()
        {
            return Err(io::Error::other(failure));
        }
        Ok(read)
    }
}

impl Drop for Body {
    fn drop(&mut self) {
        // Both fail only if curl has already gone, which is what is wanted.
        let _ = self.curl.kill();
        let _ = self.curl.wait();
    }
}

impl Transport for Curl {
    fn get(&self, url: &str) -> Result<Reply, Error> {
        let mut curl = Command::new(&self.program)
            .args(arguments(url))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Network(format!("curl: {e}")))?;
        let Some(output) = curl.stdout.take() else {
            return Err(Error::Network("curl has no output".to_owned()));
        };
        let mut body = Body {
            output: BufReader::new(output),
            curl,
            finished: false,
        };
        let Some(head) = read_head(&mut body.output) else {
            // It has ended, or is stuck writing what will never be read.
            let _ = body.curl.kill();
            let failure = body.failure();
            return Err(Error::Network(failure.unwrap_or_else(|| {
                "curl's output is not an HTTP response".to_owned()
            })));
        };
        match head.status {
            200..=299 => Ok(Reply::Body(Box::new(body))),
            300..=399 => head
                .location
                .map(Reply::Redirect)
                .ok_or(Error::Status(head.status)),
            status => Err(Error::Status(status)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::path::Path;

    use super::super::http::{self, read_limited};
    use super::*;

    const URL: &str = "https://github.com/a";

    /// A stand-in for curl in `dir`, which writes `output` to stdout and `message` to stderr and
    /// exits with `code`. It is as real a process as curl, so this tests the pipes and the exit
    /// status too, on every platform CI runs.
    fn fake_curl(dir: &Path, output: &[u8], message: &str, code: i32) -> Curl {
        let reply = dir.join("reply");
        std::fs::write(&reply, output).unwrap();
        let reply = reply.display();
        #[cfg(unix)]
        let (name, script) = (
            "curl",
            format!("#!/bin/sh\ncat '{reply}'\necho '{message}' >&2\nexit {code}\n"),
        );
        #[cfg(windows)]
        let (name, script) = (
            "curl.cmd",
            format!("@echo off\r\ntype \"{reply}\"\r\necho {message} 1>&2\r\nexit /b {code}\r\n"),
        );
        let program = dir.join(name);
        std::fs::write(&program, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(&program, permissions).unwrap();
            // A fork in another test can hold the new file open for writing until it execs.
            while Command::new(&program)
                .output()
                .is_err_and(|e| e.kind() == io::ErrorKind::ExecutableFileBusy)
            {}
        }
        Curl::with_program(program)
    }

    /// A body of `curl`'s, which must be there for a `Reply` to be one.
    fn body(reply: Result<Reply, Error>) -> Box<dyn Read + Send> {
        match reply {
            Ok(Reply::Body(body)) => body,
            Ok(Reply::Redirect(location)) => panic!("a redirect to {location}"),
            Err(e) => panic!("{e:?}"),
        }
    }

    fn head(text: &str) -> (Option<Head>, String) {
        let mut output = Cursor::new(text.as_bytes());
        let head = read_head(&mut output);
        let mut rest = String::new();
        output.read_to_string(&mut rest).unwrap();
        (head, rest)
    }

    #[test]
    fn the_command_line_trusts_nothing_but_https() {
        let arguments = arguments(URL);
        // curl only reads this as the first argument.
        assert_eq!(arguments[0], "--disable");
        assert_eq!(arguments.last().map(String::as_str), Some(URL));
        let position = arguments.iter().position(|a| a == "--proto").unwrap();
        assert_eq!(arguments[position + 1], "=https");
        for forbidden in ["--location", "-L", "--insecure", "-k", "--config"] {
            assert!(!arguments.iter().any(|a| a == forbidden), "{forbidden}");
        }
    }

    #[test]
    fn a_url_is_not_read_as_an_option_or_a_glob() {
        let arguments = arguments(URL);
        let position = arguments.iter().position(|a| a == URL).unwrap();
        assert_eq!(arguments[position - 1], "--url");
        assert!(arguments.iter().any(|a| a == "--globoff"));
    }

    #[test]
    fn a_head_ends_at_its_blank_line() {
        let (parsed, rest) = head("HTTP/1.1 200 OK\r\nContent-Type: x\r\n\r\nbody\r\n\r\nmore");
        let expected = Head {
            status: 200,
            location: None,
        };
        assert_eq!(parsed, Some(expected));
        assert_eq!(rest, "body\r\n\r\nmore");
    }

    #[test]
    fn a_redirect_has_a_location_in_any_case() {
        for name in ["location", "Location", "LOCATION"] {
            let (parsed, rest) = head(&format!("HTTP/2 302 \r\n{name}: https://a/b:c \r\n\r\n"));
            let expected = Head {
                status: 302,
                location: Some("https://a/b:c".to_owned()),
            };
            assert_eq!(parsed, Some(expected));
            assert_eq!(rest, "");
        }
    }

    #[test]
    fn a_header_that_only_contains_location_is_not_one() {
        let (parsed, _) = head("HTTP/2 302 \r\nx-location: https://a/\r\n\r\n");
        assert_eq!(parsed.unwrap().location, None);
    }

    #[test]
    fn an_interim_answer_is_skipped() {
        let (parsed, rest) = head("HTTP/2 103 \r\nlink: x\r\n\r\nHTTP/2 200 \r\n\r\nbody");
        assert_eq!(parsed.unwrap().status, 200);
        assert_eq!(rest, "body");
    }

    #[test]
    fn what_is_not_a_head_is_refused() {
        for output in [
            "",
            "garbage\r\n\r\n",
            "HTTP/1.1\r\n\r\n",
            "HTTP/1.1 abc\r\n\r\n",
            "NOPE/1.1 200 OK\r\n\r\n",
            // Cut off before its blank line.
            "HTTP/1.1 200 OK\r\nContent-Type: x\r\n",
        ] {
            assert_eq!(head(output).0, None, "{output:?}");
        }
    }

    #[test]
    fn an_endless_head_is_refused() {
        let long = format!("HTTP/1.1 200 OK\r\n{}", "x: y\r\n".repeat(20_000));
        assert_eq!(head(&long).0, None);
    }

    #[test]
    fn a_body_is_read_to_its_end() {
        let dir = tempfile::tempdir().unwrap();
        let curl = fake_curl(dir.path(), b"HTTP/2 200 \r\n\r\nthe body", "", 0);
        let mut text = String::new();
        body(curl.get(URL)).read_to_string(&mut text).unwrap();
        assert_eq!(text, "the body");
    }

    #[test]
    fn a_redirect_is_handed_back_and_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let output = b"HTTP/2 302 \r\nlocation: https://example.com/x\r\n\r\n";
        let curl = fake_curl(dir.path(), output, "", 0);
        assert!(matches!(
            curl.get(URL),
            Ok(Reply::Redirect(location)) if location == "https://example.com/x"
        ));
        // `open` is what refuses it, on the same terms as any other transport.
        assert!(matches!(http::open(&curl, URL), Err(Error::Refused(_))));
    }

    #[test]
    fn a_redirect_without_a_location_is_a_failed_status() {
        let dir = tempfile::tempdir().unwrap();
        let curl = fake_curl(dir.path(), b"HTTP/2 302 \r\n\r\n", "", 0);
        assert!(matches!(curl.get(URL), Err(Error::Status(302))));
    }

    #[test]
    fn a_failed_status_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let curl = fake_curl(dir.path(), b"HTTP/2 404 \r\n\r\nNot Found", "", 0);
        assert!(matches!(curl.get(URL), Err(Error::Status(404))));
    }

    #[test]
    fn curl_failing_before_an_answer_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let curl = fake_curl(dir.path(), b"", "SSL certificate problem", 60);
        match curl.get(URL) {
            Err(Error::Network(message)) => assert!(message.contains("certificate"), "{message}"),
            other => panic!("{:?}", other.err()),
        }
    }

    /// A body cut off by a failed connection must not pass for a whole, shorter file.
    #[test]
    fn a_cut_off_body_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let output = b"HTTP/2 200 \r\n\r\nthe start";
        let curl = fake_curl(dir.path(), output, "transfer closed", 18);
        let error = read_limited(&curl, URL, 100).unwrap_err();
        match error {
            Error::Network(message) => assert!(message.contains("transfer closed"), "{message}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn output_that_is_not_http_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let curl = fake_curl(dir.path(), b"not http\r\n", "", 0);
        assert!(matches!(curl.get(URL), Err(Error::Network(_))));
    }

    /// Without curl the check ends like any other failed request: in silence, with no prompt.
    #[test]
    fn no_curl_is_a_network_error() {
        let curl = Curl::with_program("this-curl-does-not-exist");
        assert!(matches!(curl.get(URL), Err(Error::Network(_))));
    }

    /// The real thing, by hand (`cargo test real_curl -- --ignored`): the API, and the asset
    /// redirects that the allowed hosts must cover.
    #[test]
    #[ignore = "needs the network and curl"]
    fn real_curl_serves_the_latest_release_and_its_assets() {
        let curl = Curl::new();
        let answer = read_limited(&curl, crate::update::LATEST_RELEASE_API, 4 << 20).unwrap();
        let release = crate::update::release::Release::parse(&answer).unwrap();
        let asset = release
            .assets
            .iter()
            .min_by_key(|asset| asset.size)
            .unwrap();
        let bytes = read_limited(&curl, &asset.browser_download_url, asset.size).unwrap();
        assert_eq!(bytes.len() as u64, asset.size);
    }
}
