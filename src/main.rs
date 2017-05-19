#[macro_use]
extern crate error_chain;
extern crate issuer;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate terminal_size;

pub mod cargo;

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::thread;

use cargo::Output;
use errors::*;

mod errors {
    error_chain! {
        links {
            Issuer(::issuer::Error, ::issuer::ErrorKind);
        }
        errors { BuildError }
    }
}

fn main() {
    match run() {
        Ok(()) => (),
        Err(Error(ErrorKind::BuildError, _)) => process::exit(1),
        Err(e) => {
            let stderr = io::stderr();
            let oops = "couldn't write to stderr";
            let mut log = stderr.lock();
            if let Some(backtrace) = e.backtrace() {
                writeln!(log, "\n{:?}\n", backtrace).expect(oops);
            }
            writeln!(log, "error: {}", e).expect(oops);
            for e in e.iter().skip(1) {
                writeln!(log, "caused by: {}", e).expect(oops);
            }
            drop(log);
            process::exit(1);
        }
    }
}

fn run() -> Result<()> {
    let config = Config {
        root: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
    };
    let keys = issuer::load_keys()?;
    build(&config, &keys)
}

#[derive(Clone)]
struct Config {
    root: PathBuf,
}

impl Config {
    fn client_manifest(&self) -> PathBuf {
        self.root.join("client").join("Cargo.toml")
    }
    fn driver_manifest(&self) -> PathBuf {
        self.root.join("driver").join("Cargo.toml")
    }
    fn vendor_manifest(&self) -> PathBuf {
        self.root.join("g").join("Cargo.toml")
    }
}


fn build(config: &Config, keys: &issuer::InsecureKeys) -> Result<()> {
    // G
    {
        let manifest = config.vendor_manifest();
        let stream = cargo::Command::new()
            .manifest_path(&manifest)
            .features(&["gl"])
            .spawn("build")?;
        let artifact = process_build(stream, "g", false)?;
        if artifact.fresh {
            println!("       Fresh G");
        } else {
            println!("     Rebuilt G");
            // make sure driver and client get rebuilt
            touch(&config.root.join("client/src/main.rs"))
                .chain_err(|| "touch client/src/main.rs")?;
            touch(&config.root.join("driver/src/lib.rs"))
                .chain_err(|| "touch driver/src/lib.rs")?;
            // we should write some metadata about g...?
        }
    }

    // Client thread
    let client_thread = {
        let stream = cargo::Command::new()
            .manifest_path(&config.client_manifest())
            .bin_only("client")
            .spawn("build")?;

        thread::spawn(
            move || -> Result<()> {
                let artifact = process_build(stream, "client", true)?;
                if artifact.fresh {
                    println!("       Fresh client");
                } else {
                    println!("     Rebuilt client");
                }
                Ok(())
            }
        )
    };

    // Driver
    {
        let stream = cargo::Command::new()
            .manifest_path(&config.driver_manifest())
            .spawn("build")?;
        let artifact = process_build(stream, "driver", false)?;
        if artifact.fresh {
            println!("       Fresh driver");
        } else {
            issuer::sign(&artifact.path, keys, &config.root)?;
            println!("  Signed new driver");
        }
    }

    client_thread.join().expect("client thread")?;

    Ok(())
}

#[derive(Debug)]
struct Artifact {
    path: PathBuf,
    fresh: bool,
}

fn process_build(
    stream: cargo::JsonStream,
    name: &str,
    bin: bool,
) -> Result<Artifact> {

    let mut output = None;
    let mut errored = false;

    for line in stream {
        let line = line?;
        let e = match line.decode() {
            Ok(Output::Artifact(artifact)) => {
                if artifact.target.name == name && artifact.target.kind.is_bin() == bin {
                    assert!(output.is_none(), "target {} seen twice", name);
                    assert!(
                        artifact.filenames.len() == 1,
                        "many: {:?}",
                        artifact.filenames
                    );
                    output = Some(
                        Artifact {
                            path: artifact.filenames[0].to_path_buf(),
                            fresh: artifact.fresh,
                        }
                    );
                }
                continue;
            }
            Ok(Output::Message(diag)) => {
                if diag.message.level.is_show_stopper() {
                    errored = true;
                }
                continue;
            }
            Ok(Output::BuildStep(b)) => {
                println!("  Build step {}", b.package_id);
                continue;
            }
            Err(e) => e,
        };
        cargo::log_json_error(&e, line);
    }

    if errored {
        Err(ErrorKind::BuildError.into())
    } else {
        Ok(output.unwrap_or_else(|| panic!("target {} not seen in build output", name)))
    }
}

/// Bumps an existing file's mtime.
fn touch(path: &Path) -> io::Result<()> {
    let stat = path.metadata()?;
    let len = stat.len();
    let before = stat.modified()?;
    {
        // oddly, merely opening the file for appending doesn't bump the mtime
        // this impl is portable, but has a race
        let mut f = fs::OpenOptions::new().append(true).open(path)?;
        f.write_all(b" ")?;
        f.set_len(len)?;
        f.sync_all()?;
    }
    let after = path.metadata()?.modified()?;
    if before < after {
        Ok(())
    } else {
        let msg = if before == after {
            "modification time unchanged"
        } else {
            "time travel"
        };
        Err(io::Error::new(io::ErrorKind::Other, msg))
    }
}
