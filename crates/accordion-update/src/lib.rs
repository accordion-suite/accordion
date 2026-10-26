//! Checks that release files came from Accordion's developer (decisions 9
//! and 12).
//!
//! Each release has a `SHA256SUMS` file, signed with the release key, that
//! lists the checksum of every other file in the release. Checking the
//! signature, then each file's checksum, shows that a file is what the
//! developer released, wherever it was downloaded from.

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use ring::signature::{ED25519, UnparsedPublicKey};

/// The public half of the release key, from `packaging/release-key.pem`.
pub const RELEASE_KEY: PublicKey = PublicKey([
    0x60, 0x64, 0xfe, 0x87, 0xbf, 0x82, 0x0d, 0xc4, 0xeb, 0x74, 0x61, 0xea, 0xfd, 0x70, 0xfb, 0xe9,
    0x51, 0x41, 0x81, 0x41, 0xa3, 0xa9, 0x7d, 0xf6, 0xa8, 0xd6, 0xa4, 0xf3, 0x19, 0xab, 0x1a, 0xde,
]);

/// The release's list of checksums.
pub const SUMS_FILE: &str = "SHA256SUMS";
/// The signature over [`SUMS_FILE`].
pub const SIGNATURE_FILE: &str = "SHA256SUMS.sig";
/// What the server shows about a release, listed in [`SUMS_FILE`].
pub const MANIFEST_FILE: &str = "update.json";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error(
        "The release's signature doesn't match Accordion's release key, so Accordion won't use \
         it. The download may be damaged, or someone may have changed the release"
    )]
    BadSignature,
    #[error("The release's list of files isn't in the format Accordion expects")]
    BadSums,
    #[error("{0} isn't in the release's signed list of files")]
    NotListed(String),
    #[error(
        "{0} doesn't match its checksum in the release's signed list of files. The download may \
         be damaged"
    )]
    WrongChecksum(String),
    #[error("\"{0}\" isn't a version number such as 1.2.3")]
    BadVersion(String),
}

/// An Ed25519 public key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicKey(pub [u8; 32]);

/// A release's `SHA256SUMS`, after its signature checked out.
#[derive(Clone, Debug)]
pub struct SignedSums {
    files: Vec<(String, [u8; 32])>,
}

impl SignedSums {
    /// Checks `signature` over `sums` with `key`, then reads the list.
    pub fn verify(sums: &[u8], signature: &[u8], key: &PublicKey) -> Result<Self, Error> {
        UnparsedPublicKey::new(&ED25519, key.0)
            .verify(sums, signature)
            .map_err(|_| Error::BadSignature)?;
        Self::parse(sums)
    }

    /// Reads lines in `sha256sum`'s format: the checksum in hex, two spaces,
    /// and the file's name. A `*` before the name, which `sha256sum` writes
    /// in binary mode, means the same thing.
    fn parse(sums: &[u8]) -> Result<Self, Error> {
        let text = std::str::from_utf8(sums).map_err(|_| Error::BadSums)?;
        let mut files = Vec::new();
        for line in text.lines() {
            let (hex, name) = line.split_once(' ').ok_or(Error::BadSums)?;
            let name = name.strip_prefix([' ', '*']).ok_or(Error::BadSums)?;
            // Names are files in one release folder, never paths.
            if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
                return Err(Error::BadSums);
            }
            files.push((name.to_owned(), from_hex(hex).ok_or(Error::BadSums)?));
        }
        if files.is_empty() {
            return Err(Error::BadSums);
        }
        Ok(Self { files })
    }

    /// The names of the files the list covers.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(|(name, _)| name.as_str())
    }

    /// The checksum listed for `name`.
    pub fn checksum(&self, name: &str) -> Result<[u8; 32], Error> {
        self.files
            .iter()
            .find(|(listed, _)| listed == name)
            .map(|(_, sum)| *sum)
            .ok_or_else(|| Error::NotListed(name.to_owned()))
    }

    /// Checks that `contents` is the file the list calls `name`.
    pub fn check(&self, name: &str, contents: &[u8]) -> Result<(), Error> {
        if sha256(contents) == self.checksum(name)? {
            Ok(())
        } else {
            Err(Error::WrongChecksum(name.to_owned()))
        }
    }
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    digest.as_ref().try_into().expect("SHA-256 is 32 bytes")
}

fn from_hex(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 || !hex.is_ascii() {
        return None;
    }
    let mut bytes = [0; 32];
    for (byte, pair) in bytes.iter_mut().zip(hex.as_bytes().chunks(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(bytes)
}

/// A release's version, such as `0.2.0`, or `0.3.0-rc.1` for a test
/// release. Ordered as semantic versioning orders them, so a test release
/// comes before the release it leads up to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    numbers: [u64; 3],
    pre: Vec<String>,
}

impl FromStr for Version {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Error> {
        let bad = || Error::BadVersion(text.to_owned());
        let (release, pre) = match text.split_once('-') {
            Some((release, pre)) => (release, Some(pre)),
            None => (text, None),
        };
        let mut numbers = [0; 3];
        let mut parts = release.split('.');
        for number in &mut numbers {
            let part = parts.next().ok_or_else(bad)?;
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad());
            }
            *number = part.parse().map_err(|_| bad())?;
        }
        if parts.next().is_some() {
            return Err(bad());
        }
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => {
                let parts: Vec<String> = pre.split('.').map(str::to_owned).collect();
                if parts
                    .iter()
                    .any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_alphanumeric()))
                {
                    return Err(bad());
                }
                parts
            }
        };
        Ok(Self { numbers, pre })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [major, minor, patch] = self.numbers;
        write!(f, "{major}.{minor}.{patch}")?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.numbers.cmp(&other.numbers).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => compare_pre(&self.pre, &other.pre),
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Numeric parts compare as numbers and come before words, and a longer
/// list wins a tie.
fn compare_pre(a: &[String], b: &[String]) -> Ordering {
    for (a, b) in a.iter().zip(b) {
        let order = match (a.parse::<u64>(), b.parse::<u64>()) {
            (Ok(a), Ok(b)) => a.cmp(&b),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => a.cmp(b),
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    a.len().cmp(&b.len())
}

#[cfg(test)]
mod tests {
    use base64ct::{Base64, Encoding};
    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    use super::*;

    fn key_pair() -> Ed25519KeyPair {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    }

    fn public(pair: &Ed25519KeyPair) -> PublicKey {
        PublicKey(pair.public_key().as_ref().try_into().unwrap())
    }

    fn sums_for(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut sums = String::new();
        for (name, contents) in files {
            let hex: String = sha256(contents)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            sums.push_str(&format!("{hex}  {name}\n"));
        }
        sums.into_bytes()
    }

    #[test]
    fn the_release_key_matches_the_one_in_packaging() {
        let pem = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packaging/release-key.pem"
        ))
        .unwrap();
        let line = pem.lines().find(|line| !line.starts_with("-----")).unwrap();
        let der = Base64::decode_vec(line).unwrap();
        // An Ed25519 public key's DER is 12 bytes of header, then the key.
        assert_eq!(der.len(), 44);
        assert_eq!(der[12..], RELEASE_KEY.0);
    }

    #[test]
    fn trusts_files_listed_in_signed_sums() {
        let pair = key_pair();
        let sums = sums_for(&[
            ("update.json", b"{}"),
            ("accordion_0.2.0_arm64.deb", b"deb"),
        ]);
        let signature = pair.sign(&sums);
        let signed = SignedSums::verify(&sums, signature.as_ref(), &public(&pair)).unwrap();

        assert_eq!(
            signed.files().collect::<Vec<_>>(),
            ["update.json", "accordion_0.2.0_arm64.deb"]
        );
        signed.check("update.json", b"{}").unwrap();
        assert_eq!(
            signed.check("update.json", b"{ }"),
            Err(Error::WrongChecksum("update.json".into()))
        );
        assert_eq!(
            signed.check("install.sh", b""),
            Err(Error::NotListed("install.sh".into()))
        );
    }

    #[test]
    fn refuses_sums_signed_with_another_key_or_changed() {
        let pair = key_pair();
        let sums = sums_for(&[("update.json", b"{}")]);
        let signature = pair.sign(&sums);

        let other = key_pair();
        assert!(matches!(
            SignedSums::verify(&sums, signature.as_ref(), &public(&other)),
            Err(Error::BadSignature)
        ));
        let mut changed = sums.clone();
        changed[0] ^= 1;
        assert!(matches!(
            SignedSums::verify(&changed, signature.as_ref(), &public(&pair)),
            Err(Error::BadSignature)
        ));
        assert!(matches!(
            SignedSums::verify(&sums, b"short", &public(&pair)),
            Err(Error::BadSignature)
        ));
        assert!(matches!(
            SignedSums::verify(&sums, signature.as_ref(), &RELEASE_KEY),
            Err(Error::BadSignature)
        ));
    }

    #[test]
    fn reads_sha256sum_output_and_nothing_else() {
        let hex = "ab".repeat(32);
        let parse = |text: String| SignedSums::parse(text.as_bytes()).map(|s| s.files.len());
        assert_eq!(parse(format!("{hex}  install.sh\n")), Ok(1));
        assert_eq!(parse(format!("{hex} *install.sh\n")), Ok(1));
        assert_eq!(parse(format!("{hex} install.sh\n")), Err(Error::BadSums));
        assert_eq!(
            parse(format!("{hex}  ../install.sh\n")),
            Err(Error::BadSums)
        );
        assert_eq!(parse(format!("{hex}  .hidden\n")), Err(Error::BadSums));
        assert_eq!(
            parse(format!("{}  install.sh\n", "zz".repeat(32))),
            Err(Error::BadSums)
        );
        assert_eq!(
            parse(format!("{}  install.sh\n", &hex[2..])),
            Err(Error::BadSums)
        );
        assert_eq!(parse(String::new()), Err(Error::BadSums));
    }

    #[test]
    fn orders_versions_like_semantic_versioning() {
        let v = |text: &str| text.parse::<Version>().unwrap();
        let ordered = [
            "0.1.0",
            "0.2.0-alpha",
            "0.2.0-alpha.1",
            "0.2.0-alpha.beta",
            "0.2.0-rc.1",
            "0.2.0-rc.2",
            "0.2.0-rc.10",
            "0.2.0",
            "0.2.1",
            "0.10.0",
            "1.0.0",
        ];
        for pair in ordered.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        }
        assert_eq!(v("0.2.0"), v("0.2.0"));
        assert_eq!(v("0.3.0-rc.1").to_string(), "0.3.0-rc.1");
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "v1.2.3",
            "1.2.x",
            "1.2.3-",
            "1.2.3-a..b",
            "1.-2.3",
        ] {
            assert!(bad.parse::<Version>().is_err(), "{bad}");
        }
    }
}
