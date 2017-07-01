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
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use issuer::{Bincoded, DriverInfo};

use cargo::Output;
use errors::*;

pub mod errors {
    error_chain! {
        links {
            Issuer(::issuer::Error, ::issuer::ErrorKind);
        }
        errors { BuildError Cancelled }
    }
}

fn main() {
    let oops = "couldn't write to stderr";
    let mut stderr = io::stderr();
    match run() {
        Ok(()) => (),
        Err(Error(ErrorKind::Issuer(issuer::ErrorKind::InvalidPassword), _)) => {
            writeln!(stderr, "Invalid encryption password.").expect(oops);
            process::exit(1);
        }
        Err(e) => {
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

    let mut input = format!("\n");
    while input != "" && input != "q\n" {
        if input == "?\n" {
            help();
            continue
        }

        match build(&config, &keys) {
            Ok((info, novelty)) => {
                if input == "f\n" || !novelty.is_still_fresh() {
                    let hex = info.digest.short_hex();
                    match announce_build(info) {
                        Ok(()) => println!("   Announced driver {}", hex),
                        Err(e) => writeln!(io::stderr(), "announce: {}", e).expect("stderr"),
                    }
                }
            }
            Err(Error(ErrorKind::BuildError, _)) |
            Err(Error(ErrorKind::Cancelled, _)) => (),
            Err(e) => return Err(e)
        }

        print!("> ");
        io::stdout().flush().expect("flush");
        input.clear();
        io::stdin().read_line(&mut input).chain_err(|| "stdin")?;
    }

    fn help() {
        println!("
? - help
f - rebuild and force re-announce
q - quit
anything else: rebuild and announce if changed
");
    }

    Ok(())
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


fn build(config: &Config, keys: &issuer::InsecureKeys) -> Result<(DriverInfo, Novelty)> {
    // G
    {
        let manifest = config.vendor_manifest();
        let stream = cargo::Command::new()
            .manifest_path(&manifest)
            .features(&["gl"])
            .spawn("build")?;
        let artifact = process_build(stream, "g", false, None)?;
        if artifact.novelty.is_still_fresh() {
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

    let client_switch = DeadMansSwitch::new();
    let client_thread = {
        let cancel_flag = client_switch.0.clone();
        let stream = cargo::Command::new()
            .manifest_path(&config.client_manifest())
            .bin_only("client")
            .spawn("build")?;

        thread::spawn(
            move || -> Result<()> {
                let artifact = process_build(stream, "client", true, Some(&cancel_flag))?;
                if artifact.novelty.is_still_fresh() {
                    println!("       Fresh client");
                } else {
                    println!("     Rebuilt client");
                }
                Ok(())
            }
        )
    };

    // Driver
    let descriptor;
    let novelty;
    {
        let stream = cargo::Command::new()
            .manifest_path(&config.driver_manifest())
            .spawn("build")?;
        let artifact = process_build(stream, "driver", false, None)?;
        novelty = artifact.novelty;
        match novelty {
            Novelty::StillFresh => {
                descriptor = issuer::verify(&keys.0, &config.root)?;
                println!("       Fresh driver");
            }
            Novelty::BrandNew => {
                descriptor = issuer::sign(&artifact.path, keys, &config.root)?;
                println!("  Signed new driver");
            }
        }
    }

    client_thread.join().expect("client thread")?;
    drop(client_switch);

    Ok((descriptor, novelty))
}

#[derive(Debug)]
struct Artifact {
    path: PathBuf,
    novelty: Novelty,
}

#[derive(Debug)]
enum Novelty {
    BrandNew,
    StillFresh,
}

impl Novelty {
    fn is_still_fresh(&self) -> bool {
        match *self {
            Novelty::BrandNew => false,
            Novelty::StillFresh => true,
        }
    }
}

fn process_build(
    stream: cargo::JsonStream,
    name: &str,
    bin: bool,
    kill_switch: Option<&AtomicBool>,
) -> Result<Artifact> {

    let mut stderr = io::stderr();
    let oops = "couldn't write to stderr";

    let mut output = None;
    let mut errored = false;
    let mut logged_json = false;

    for line in stream {
        if kill_switch.map(|b| b.load(Ordering::Relaxed)).unwrap_or(false) {
            return Err(ErrorKind::Cancelled.into())
        }
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
                            novelty: if artifact.fresh {
                                Novelty::StillFresh
                            } else {
                                Novelty::BrandNew
                            },
                        }
                    );
                }
                continue;
            }
            Ok(Output::Message(msg)) => {
                let diag = msg.message;
                if diag.level.is_show_stopper() {
                    errored = true;
                }
                let mut out = stderr.lock();
                write!(out, "{}", diag.level).expect(oops);
                if let Some(code) = diag.code {
                    write!(out, "[{}]", code.code).expect(oops);
                }
                writeln!(out, ": {}", diag.message).expect(oops);
                if !diag.spans.is_empty() {
                    // probably want to search for the span where .is_primary
                    let ref span = diag.spans[0];
                    writeln!(
                        out,
                        "   --> {}:{}:{}",
                        span.file_name.display(),
                        span.line_start,
                        span.column_start
                    ).expect(oops);
                }
                writeln!(out, "").expect(oops);
                continue;
            }
            Ok(Output::BuildStep(b)) => {
                println!("  Build step {}", b.package_id);
                continue;
            }
            Err(e) => e,
        };

        if logged_json {
            writeln!(stderr, "While parsing JSON:\n    {}\n", e).expect(oops);
        } else {
            cargo::log_json_error(&e, line);
            logged_json = true;
        }
    }

    if errored {
        Err(ErrorKind::BuildError.into())
    } else {
        Ok(output.unwrap_or_else(|| panic!("target {} not seen in build output", name)))
    }
}

fn announce_build(info: DriverInfo) -> Result<()> {
    let addr: SocketAddr = ([127, 0, 0, 1], 2002).into();
    let mut sock = TcpStream::connect(addr).chain_err(|| "couldn't connect to server")?;
    let buf = Bincoded::new(&info).chain_err(|| "couldn't serialize driver descriptor")?;
    write_with_length_sync(&mut sock, buf.as_ref())
        .chain_err(|| "couldn't write driver descriptor")
}

fn write_with_length_sync<W: Write>(writer: &mut W, bytes: &[u8]) -> io::Result<()> {
    let len = bytes.len();
    if len > 0xffff {
        let msg = format!("written message too long: {}", len);
        return Err(io::Error::new(io::ErrorKind::Other, msg));
    }
    let len_buf = [(len >> 8) as u8, len as u8];
    writer.write_all(&len_buf)?;
    writer.write_all(bytes)
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

#[derive(Debug, Default)]
pub struct DeadMansSwitch(pub Arc<AtomicBool>);

impl DeadMansSwitch {
    pub fn new() -> Self {
        Default::default()
    }
}

impl Drop for DeadMansSwitch {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed)
    }
}
