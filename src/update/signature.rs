//! Verifying a download against the public key embedded in the app.
//!
//! Each release asset has a detached `<asset>.sig`: the 64-byte ed25519 signature of
//! [`signed_message`], which holds the asset's name and SHA-256 rather than the file itself, so
//! a file of any size is verified without loading it into memory. Because the name carries the
//! version, an older asset (validly signed, but under its own name) cannot pass as the latest.
//! `scripts/sign-update-assets.sh` writes these files.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use ring::digest;
use ring::signature::{ED25519, UnparsedPublicKey};

use super::Error;

/// `resources/update-signing.pub`, which holds a placeholder until the real key is committed.
const EMBEDDED_KEY: &str = include_str!("../../resources/update-signing.pub");

/// Separates these signatures from anything else the key might ever sign.
const CONTEXT: &str = "open-draw-chemistry-update-v1";

/// An ed25519 public key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    /// The key in a `update-signing.pub` file: `#` comment lines, then the 64 hex digits of the
    /// raw key. Anything else, the placeholder included, is no key at all.
    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'));
        let (Some(hex), None) = (lines.next(), lines.next()) else {
            return None;
        };
        if hex.len() != 64 || !hex.is_ascii() {
            return None;
        }
        let mut key = [0; 32];
        for (byte, digits) in key.iter_mut().zip(hex.as_bytes().chunks(2)) {
            let digits = std::str::from_utf8(digits).ok()?;
            *byte = u8::from_str_radix(digits, 16).ok()?;
        }
        (key != [0; 32]).then_some(Self(key))
    }

    /// The key built into the app; `None` while it is still the placeholder, which turns
    /// automatic installs off.
    pub fn embedded() -> Option<Self> {
        Self::parse(EMBEDDED_KEY)
    }
}

/// What a signature covers.
pub fn signed_message(asset_name: &str, sha256: &str) -> String {
    format!("{CONTEXT}\n{asset_name}\n{sha256}\n")
}

/// The SHA-256 of the file at `path`, in lower-case hex.
fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut context = digest::Context::new(&digest::SHA256);
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        context.update(&buffer[..read]);
    }
    Ok(context
        .finish()
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Check the file at `path`, which claims to be the asset `asset_name`, against `signature`.
pub fn verify_file(
    key: &PublicKey,
    asset_name: &str,
    path: &Path,
    signature: &[u8],
) -> Result<(), Error> {
    let message = signed_message(asset_name, &sha256_file(path)?);
    UnparsedPublicKey::new(&ED25519, key.0)
        .verify(message.as_bytes(), signature)
        .map_err(|_| Error::Signature)
}

#[cfg(test)]
mod tests {
    use ring::signature::{Ed25519KeyPair, KeyPair};

    use super::*;

    /// A key pair for these tests only, from a fixed seed.
    fn test_keys(seed: u8) -> (Ed25519KeyPair, PublicKey) {
        let pair = Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap();
        let public = PublicKey(pair.public_key().as_ref().try_into().unwrap());
        (pair, public)
    }

    /// `name` holding `bytes` in a fresh directory, and the signature `keys` give it.
    fn signed_file(
        keys: &Ed25519KeyPair,
        name: &str,
        bytes: &[u8],
    ) -> (tempfile::TempDir, std::path::PathBuf, Vec<u8>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        let message = signed_message(name, &sha256_file(&path).unwrap());
        let signature = keys.sign(message.as_bytes()).as_ref().to_vec();
        (dir, path, signature)
    }

    #[test]
    fn a_correct_signature_verifies() {
        let (keys, public) = test_keys(1);
        let (_dir, path, signature) = signed_file(&keys, "app-1.0.0.exe", b"installer");
        assert!(verify_file(&public, "app-1.0.0.exe", &path, &signature).is_ok());
    }

    #[test]
    fn a_tampered_file_fails() {
        let (keys, public) = test_keys(1);
        let (_dir, path, signature) = signed_file(&keys, "app-1.0.0.exe", b"installer");
        std::fs::write(&path, b"Installer").unwrap();
        assert!(matches!(
            verify_file(&public, "app-1.0.0.exe", &path, &signature),
            Err(Error::Signature)
        ));
    }

    #[test]
    fn a_tampered_signature_fails() {
        let (keys, public) = test_keys(1);
        let (_dir, path, mut signature) = signed_file(&keys, "app-1.0.0.exe", b"installer");
        signature[10] ^= 1;
        assert!(verify_file(&public, "app-1.0.0.exe", &path, &signature).is_err());
    }

    #[test]
    fn a_signature_of_the_wrong_length_fails() {
        let (keys, public) = test_keys(1);
        let (_dir, path, signature) = signed_file(&keys, "app-1.0.0.exe", b"installer");
        assert!(verify_file(&public, "app-1.0.0.exe", &path, &signature[..63]).is_err());
        assert!(verify_file(&public, "app-1.0.0.exe", &path, &[]).is_err());
    }

    #[test]
    fn the_wrong_key_fails() {
        let (keys, _) = test_keys(1);
        let (_, other) = test_keys(2);
        let (_dir, path, signature) = signed_file(&keys, "app-1.0.0.exe", b"installer");
        assert!(verify_file(&other, "app-1.0.0.exe", &path, &signature).is_err());
    }

    /// A validly signed older asset cannot be served as the latest: the signature is bound to
    /// the name, and the app only looks for the name of the version it was told about.
    #[test]
    fn a_signature_for_another_asset_name_fails() {
        let (keys, public) = test_keys(1);
        let (_dir, path, signature) = signed_file(&keys, "app-1.0.0.exe", b"old and vulnerable");
        assert!(verify_file(&public, "app-2.0.0.exe", &path, &signature).is_err());
    }

    /// The format `scripts/sign-update-assets.sh` writes, from OpenSSL rather than ring: the
    /// keys here are a throwaway pair that signs nothing else.
    #[test]
    fn a_signature_made_by_the_release_script_verifies() {
        let public = PublicKey::parse(SCRIPT_PUBLIC_KEY).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SCRIPT_ASSET_NAME);
        std::fs::write(&path, SCRIPT_ASSET).unwrap();
        let signature: Vec<u8> = (0..SCRIPT_SIGNATURE.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&SCRIPT_SIGNATURE[i..i + 2], 16).unwrap())
            .collect();
        verify_file(&public, SCRIPT_ASSET_NAME, &path, &signature).unwrap();
    }

    const SCRIPT_ASSET_NAME: &str = "open-draw-chemistry-9.9.9-windows-x64-setup.exe";
    const SCRIPT_ASSET: &[u8] = b"not really an installer\n";
    const SCRIPT_PUBLIC_KEY: &str =
        "05e58c50b210423d689e68ee96fd75f2d6d797c58e04ddeb8cd49c3b925b3f83";
    const SCRIPT_SIGNATURE: &str = concat!(
        "df9454eb608ba41668dad0def8a42ad40aee4cc5db662abd4e0999fae8a34918",
        "6d6dcf129b018974e2164eb168036d7731bc42b487fd50f07d4a93c9453a9703",
    );

    #[test]
    fn the_key_file_format() {
        let (_, public) = test_keys(3);
        let hex: String = public.0.iter().map(|b| format!("{b:02x}")).collect();
        let upper = hex.to_uppercase();
        assert_eq!(PublicKey::parse(&hex), Some(public));
        assert_eq!(PublicKey::parse(&upper), Some(public));
        let commented = format!("# a comment\n\n  {hex}  \n");
        assert_eq!(PublicKey::parse(&commented), Some(public));
        assert_eq!(PublicKey::parse(&format!("{hex}\n{hex}")), None);
        assert_eq!(PublicKey::parse(&hex[1..]), None);
        assert_eq!(PublicKey::parse(&format!("{}gg", &hex[..62])), None);
    }

    #[test]
    fn the_placeholder_is_no_key() {
        assert_eq!(PublicKey::parse(""), None);
        assert_eq!(PublicKey::parse("# only a comment\n"), None);
        assert_eq!(PublicKey::parse("PLACEHOLDER\n"), None);
        assert_eq!(PublicKey::parse(&"0".repeat(64)), None);
        // Non-ASCII text of the right byte length must not split a character.
        assert_eq!(PublicKey::parse(&"é".repeat(32)), None);
    }
}
