#![feature(core_intrinsics)]
#![recursion_limit = "1024"]

pub extern crate bincode;
extern crate bytes;
extern crate digest as digest_crate;
#[macro_use]
extern crate error_chain;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate sha3;

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io;
use std::io::prelude::*;
use std::path::{Component, Path, PathBuf};

pub mod bincoded;
pub mod digest;

pub use digest::Digest;
pub use errors::*;

pub mod errors {
    use std::io;

    error_chain! {
        foreign_links {
            Io(io::Error);
        }
    }
}

pub struct Dag {
    objs: PathBuf,
    roots: PathBuf,
}

impl Dag {
    pub fn new<P: AsRef<Path>>(dir: P) -> Result<Self> {

        fn mkdir(dir: PathBuf) -> Result<PathBuf> {
            match dir.metadata() {
                Ok(meta) => {
                    ensure!(meta.is_dir(), "not a directory");
                    Ok(dir)
                }
                Err(ref e) if e.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir_all(&dir)?;
                    Ok(dir)
                }
                Err(e) => Err(e.into()),
            }
        }

        let dir = dir.as_ref();
        let objs = mkdir(dir.join("o"))?;
        let roots = mkdir(dir.join("r"))?;
        Ok(Dag { objs, roots })
    }

    pub fn save(&self, bytes: &[u8]) -> Result<Digest> {
        use std::os::unix::ffi::OsStrExt;

        let digest = Digest::from_bytes(bytes);
        let path = self.objs.join(OsStr::from_bytes(&digest.hex_bytes()));
        // XXX write to temp dir then move!
        let mut f = File::create(path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(digest)
    }

    pub fn set_root(&self, id: &str, digest: &Digest) -> Result<()> {
        use std::os::unix::ffi::OsStrExt;

        validate_root_name(Path::new(id))?;
        let link = self.roots.join(id);

        let mut obj = PathBuf::new(); // capacity?
        obj.push(Component::ParentDir.as_ref());
        obj.push("o");
        obj.push(OsStr::from_bytes(&digest.hex_bytes()));
        //let target = self.objs.join(OsStr::from_bytes(&digest.hex_bytes()));

        std::os::unix::fs::symlink(obj, link).chain_err(|| format!("symlink {:?}", id))?;
        Ok(())
    }

    pub fn root(&self, id: &str) -> Result<Option<Digest>> {
        let name = Path::new(id);
        validate_root_name(name)?;
        let target_path = match fs::read_link(self.roots.join(name)) {
            Err(ref e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            s => s,
        }?;
        let mut cs = target_path.components();
        ensure!(cs.next().map(|c| c == Component::ParentDir).unwrap_or(false),
                "root {:?} is corrupt (..)", id);
        ensure!(cs.next().map(|c| c == Component::Normal("o".as_ref())).unwrap_or(false),
                "root {:?} is corrupt (o)", id);
        let digest = cs.next()
            .ok_or_else(|| format!("root {:?} is corrupt (missing hex)", id))
            .and_then(|c| {
                match c {
                    Component::Normal(os) => os.to_str().ok_or_else(|| "bad utf8".into()),
                    _ => Err(format!("root {:?} is corrupt (bad hex)", id).into()),
                }
            })
            .and_then(|s| s.parse::<Digest>().map_err(|()| format!("root {:?} bad hex", id)))?;

        ensure!(cs.next().is_none(), "root {:?} is corrupt (trailer)");
        Ok(Some(digest))
    }
}

fn validate_root_name(name: &Path) -> Result<()> {
    let mut cs = name.components();
    match cs.next() {
        Some(Component::Normal(_)) => (),
        _ => bail!("{:?} is not a filename", name),
    }
    ensure!(cs.next().is_none(), "{:?} contains slashes", name);
    Ok(())
}

#[cfg(test)]
mod tests {
    extern crate tempdir;

    use self::tempdir::TempDir;

    use super::Dag;

    #[test]
    fn smoke() {
        let dir = TempDir::new("dag_smoke").unwrap();
        let dag = Dag::new(dir.path()).unwrap();
        ::std::mem::forget(dir); // TEMP
        assert_eq!(dag.root("404").expect("404"), None);
        assert!(dag.root("/").is_err());

        let dest = dag.save(&[1, 2, 3]).expect("123");
        dag.set_root("abc", &dest).unwrap();
        assert_eq!(dag.root("abc").expect("abc"), Some(dest));
    }
}
