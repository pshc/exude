use std::fs::{self, File};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::str;

use futures::{Future, future};
use futures_cpupool::CpuPool;
use sodiumoxide::crypto::sign::{self, PublicKey};
use tokio_io::{self, AsyncRead};

use common::{self, IoFuture};
use proto::{DriverInfo, handshake};

/// Generated by `cd issuer; cargo run -- keygen`.
pub static PUBLIC_KEY: PublicKey = PublicKey(*include_bytes!("../../issuer/cred/public"));


/// Downloads the newest driver (if needed), returning its path.
pub fn fetch_driver<R: AsyncRead + 'static>(reader: R) -> IoFuture<(R, Box<DriverInfo>, PathBuf)> {
    box common::read_bincoded(reader).and_then(
        |(reader, welcome)| -> IoFuture<_> {

            match welcome {
                handshake::Welcome::Current => unimplemented!(),
                handshake::Welcome::InlineDriver(info) => {
                    println!(
                        "receiving driver {} ({}kb)",
                        info.digest.short_hex(),
                        info.len / 1000
                    );

                    let download = verify_and_save(box info, reader).and_then(Ok);

                    box download
                }
                handshake::Welcome::DownloadDriver(url, info) => {
                    println!("TODO download {} and check {} and SIG", url, info.digest);
                    let download = io::Error::new(ErrorKind::Other, "todo download");
                    box future::failed(download)
                }
            }
        },
    )
}

/// Intended for inline downloads only; use a smart HTTP client for larger downloads.
fn verify_and_save<R: AsyncRead + 'static>(
    info: Box<DriverInfo>,
    reader: R,
) -> IoFuture<(R, Box<DriverInfo>, PathBuf)> {
    let len = info.len;
    if len > handshake::INLINE_MAX {
        let err = io::Error::new(ErrorKind::InvalidInput, "inline download too large");
        return box future::failed(err);
    }

    let mut path = repo_path().to_owned();
    let hex = info.digest.hex_bytes();
    path.push(unsafe { str::from_utf8_unchecked(&hex) });

    // xxx not a fan of reading the whole thing into memory... we could mmap?
    // or if we had a streaming version of `sign::verify_detached`, we could stream
    let mut buf = Vec::with_capacity(len);
    unsafe {
        buf.set_len(len);
    }
    box tokio_io::io::read_exact(reader, buf).and_then(
        |(reader, buf)| {
            let future = future::lazy(
                move || {
                    let checked_digest = utils::digest_from_bytes(&buf[..]);
                    if info.digest != checked_digest {
                        let err = io::Error::new(ErrorKind::InvalidData, "hash check failed");
                        return Err(err);
                    }

                    // we *could* parallelize this with hashing...
                    let sig = sign::Signature(info.sig.0);
                    if !sign::verify_detached(&sig, &buf, &PUBLIC_KEY) {
                        let err = io::Error::new(ErrorKind::InvalidData, "sig check failed");
                        return Err(err);
                    }

                    let mut file = File::create(&path)?;
                    file.write_all(&buf)?;
                    file.sync_data()?;

                    Ok((info, path))
                },
            );

            CpuPool::new(1)
                .spawn(future)
                .map(|(info, path)| (reader, info, path))
        },
    )
}

fn repo_path() -> &'static Path {
    use std::sync::{ONCE_INIT, Once};

    static MKDIR: Once = ONCE_INIT;
    static mut PATH: Option<PathBuf> = None;

    MKDIR.call_once(
        || {
            let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            path.push("repo");

            match fs::create_dir(&path) {
                Ok(()) => (),
                Err(ref e) if e.kind() == io::ErrorKind::AlreadyExists => (),
                Err(e) => panic!("couldn't create {:?}: {}", path, e),
            }

            unsafe { PATH = Some(path) }
        },
    );

    unsafe { PATH.as_ref().unwrap() }
}

#[test]
fn test_repo_path() {
    let path = repo_path();
    assert!(path.ends_with("repo"));
    assert!(path.parent().unwrap().exists());
}

pub mod utils {
    use std::fmt;
    use std::time::{Duration, Instant};
    use sha3::Shake128;
    use digest::{Input, VariableOutput};

    use proto::{Digest, digest};

    /// Hash a byte slice to our concrete 256-bit Digest type.
    ///
    /// This is temporary; we usually want to hash as data streams in, without waiting to buffer.
    pub fn digest_from_bytes(bytes: &[u8]) -> Digest {

        let before = Instant::now();

        let mut hasher = Shake128::default();
        hasher.digest(bytes);
        let mut result = [0u8; digest::LEN];
        hasher.variable_result(&mut result).unwrap();
        let digest = Digest(result);

        println!(
            "hashed {}kb in {}: {}",
            bytes.len() / 1000,
            PrettyDuration(&before.elapsed()),
            digest.short_hex()
        );

        digest
    }

    pub struct PrettyDuration<'a>(&'a Duration);

    impl<'a> fmt::Display for PrettyDuration<'a> {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            let s = self.0.as_secs();
            let ns = self.0.subsec_nanos();
            // note: rounds down
            if s > 9 {
                write!(f, "{}.{}s", s, ns / 100_000_000)
            } else {
                write!(f, "{}.{:03}s", s, ns / 1_000_000)
            }
        }
    }
}
