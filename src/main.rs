extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate terminal_size;

pub mod cargo;

use std::io;
use std::path::PathBuf;

use cargo::Output;

fn main() {
    let config = Config {
        root: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
    };
    if let Err(e) = build_driver(&config) {
        println!("Error: {}", e);
    }
}

struct Config {
    root: PathBuf,
}

impl Config {
    fn vendor_manifest(&self) -> PathBuf {
        self.root.join("g").join("Cargo.toml")
    }
}


fn build_driver(config: &Config) -> io::Result<()> {

    let print_src = |target: &cargo::Target| {
        let src = target.src_path;
        let src = src.strip_prefix(&config.root).unwrap_or(src);
        println!("{}", src.display());
    };

    let g_manifest = config.vendor_manifest();
    let stream = cargo::Command::new()
        .manifest_path(&g_manifest)
        .features(&["gl"])
        .spawn("build")?;

    for line in stream {
        let line = line?;
        let e = match line.decode() {
            Ok(Output::Artifact(artifact)) => {
                if !artifact.fresh {
                    print_src(&artifact.target);
                }
                continue;
            }
            Ok(Output::Message(diag)) => {
                print!("{} in ", diag.message.level);
                print_src(&diag.target);
                println!(" -> {}", diag.message.message);
                continue;
            }
            Ok(Output::BuildStep(_)) => continue,
            Err(e) => e,
        };
        cargo::log_json_error(&e, line);
    }
    Ok(())
}
