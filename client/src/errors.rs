use std::io::{self, Write};

use g::gfx_text;

use proto;

error_chain! {
    errors {
        BrokenComms {}
        /// Wrapped since `gfx_text::Error` doesn't impl Error
        Text(t: gfx_text::Error) {}
    }
    foreign_links {
        Bincode(proto::bincoded::Error);
    }
}

pub fn display_net_thread_error(e: Error) -> io::Result<()> {
    let mut stderr = io::stderr();

    match *e.kind() {
        ErrorKind::BrokenComms => writeln!(stderr, "net: broken comms")?,
        _ => {
            let mut log = stderr.lock();
            if let Some(backtrace) = e.backtrace() {
                writeln!(log, "\n{:?}\n", backtrace)?;
            }
            writeln!(log, "net: {}", e)?;
            for e in e.iter().skip(1) {
                writeln!(log, "caused by: {}", e)?;
            }
        }
    }
    Ok(())
}
