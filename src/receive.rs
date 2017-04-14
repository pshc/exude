use std::fs::{self, File};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

use futures::Future;
use futures::future;
use tokio_io::{self, AsyncRead};

use common::Digest;

/// Maximum byte length of an InlineDriver payload.
pub static INLINE_MAX: usize = 100_000_000;

/// Intended for inline downloads only; use a smart HTTP client for larger downloads.
///
/// Temporary: return type is boxed instead of `impl Future` due to rust ICE #37096
pub fn verify_and_save<R: AsyncRead + 'static>(len: usize, digest: Digest, reader: R)
    -> Box<Future<Item=(R, Digest, PathBuf), Error=io::Error>>
{
    if len > INLINE_MAX {
        let err = io::Error::new(ErrorKind::InvalidInput, "inline download too large");
        return box future::failed(err)
    }

    let mut path = repo_path().to_owned();
    // i feel like we could avoid constructing a string here...
    path.push(format!("{}", digest));

    // xxx--don't buffer! stream into another thread
    let mut buf = Vec::with_capacity(len);
    unsafe {
        buf.set_len(len);
    }
    box tokio_io::io::read_exact(reader, buf).and_then(|(reader, buf)| {
        let checked_digest = utils::digest_from_bytes(&buf[..]);
        if digest != checked_digest {
            let err = io::Error::new(ErrorKind::InvalidData, "hash check failed");
            return Err(err)
        }

        // xxx we'll just write synchronously to start
        let mut file = File::create(&path)?;
        file.write_all(&buf)?;
        file.sync_data()?;

        Ok((reader, digest, path))
    })
}

fn repo_path() -> &'static Path {
    use std::sync::{Once, ONCE_INIT};

    static MKDIR: Once = ONCE_INIT;
    static mut PATH: Option<PathBuf> = None;

    MKDIR.call_once(|| {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push("repo");

        match fs::create_dir(&path) {
            Ok(()) => (),
            Err(ref e) if e.kind() == io::ErrorKind::AlreadyExists => (),
            Err(e) => panic!("couldn't create {:?}: {}", path, e)
        }

        unsafe {
            PATH = Some(path)
        }
    });

    unsafe {
        PATH.as_ref().unwrap()
    }
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
    use sha3::Shake256;
    use digest::{Input, VariableOutput};

    use common;

    /// Hash a byte slice to our concrete 512-bit Digest type.
    ///
    /// This is temporary; we usually want to hash as data streams in, without waiting to buffer.
    pub fn digest_from_bytes(bytes: &[u8]) -> common::Digest {

        let before = Instant::now();

        let mut hasher = Shake256::default();
        hasher.digest(bytes);
        let mut result = [0u8; 64];
        hasher.variable_result(&mut result).unwrap();
        let digest = common::Digest(result);

        println!("hashed {}kb in {}: {}", bytes.len()/1000, PrettyDuration(&before.elapsed()),
                digest.short_hex());

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
