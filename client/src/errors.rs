use std::io::{self, Write};

use g::gfx_text;

use proto;
use hyper;

error_chain! {
    errors {
        BrokenComms {}
        /// Wrapped since `gfx_text::Error` doesn't impl Error
        Text(t: gfx_text::Error) {}
    }
    foreign_links {
        Bincode(proto::bincoded::Error);
        Hyper(hyper::Error);
        Io(io::Error);
    }
}

pub fn display_net_thread_error(e: Error) -> io::Result<()> {
    let mut stderr = io::stderr();

    match *e.kind() {
        ErrorKind::BrokenComms => writeln!(stderr, "net: broken comms")?,
        _ => {
            let mut log = stderr.lock();
            writeln!(log, "net: {}", e)?;
            for e in e.iter().skip(1) {
                writeln!(log, "caused by: {}", e)?;
            }
        }
    }
    Ok(())
}
