#[macro_use]
extern crate error_chain;
extern crate issuer;

use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process;

use issuer::errors::*;
use issuer::Secret;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        usage();
    }

    if let Err(ref e) = dispatch(&args[1..]) {
        let stderr = &mut io::stderr();
        let errmsg = "Error writing to stderr";

        writeln!(stderr, "error: {}", e).expect(errmsg);

        for e in e.iter().skip(1) {
            writeln!(stderr, "caused by: {}", e).expect(errmsg);
        }

        process::exit(1);
    }
}

fn dispatch(args: &[String]) -> Result<()> {
    match &*args[0] {
        "keygen" => keygen(),
        "sign" => sign(),
        cmd => {
            let _ = writeln!(io::stderr(), "Unknown command: {}", cmd);
            usage()
        }
    }
}

fn keygen() -> Result<()> {
    let dir = issuer::cred_path()?;
    println!("Keys will be written into: {}", dir.display());

    let password;
    {
        password = Secret::from_user_input("Please choose an encryption passphrase: ")?;
        let again = Secret::from_user_input("Please repeat it: ")?;
        ensure!(password == again, "passwords did not match");
    }

    issuer::keygen(&dir, password)
}

fn sign() -> Result<()> {
    let mut root_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root_path.pop();
    let root_path = root_path;

    let mut driver_path = root_path.clone();
    driver_path.push("driver");
    driver_path.push("target");
    driver_path.push("debug"); // xxx
    driver_path.push("libdriver.dylib"); // xxx

    let keys = issuer::load_keys()?;

    issuer::sign(&driver_path, &keys, &root_path).map(|_info| ())
}

fn usage() -> ! {
    println!(
        "Command patterns:
    keygen
    sign
"
    );
    process::exit(1)
}
