use std;
use std::io::{self, Write};
use std::path::Path;
use std::sync::mpsc;

use futures;
use libc::c_void;
use libloading::{Library, Symbol};

use driver_abi::{DriverCallbacks, DriverCtx};
use g;

rental! {
    mod rent_libloading {
        use g;
        use libloading::{Library, Symbol};

        #[rental]
        pub struct RentDriver {
            lib: Box<Library>,
            syms: (Symbol<'lib, g::GlDrawFn>,
                   Symbol<'lib, g::GlSetupFn>,
                   Symbol<'lib, g::GlCleanupFn>),
        }
    }
}

pub struct Driver(rent_libloading::RentDriver);

impl g::GlInterface for Driver {
    fn draw(&self, ctx: &g::GlCtx, encoder: &mut g::Encoder) {
        self.0.rent(|syms| (syms.0)(ctx, encoder))
    }

    fn setup(&self, f: &mut g::Factory, rtv: g::RenderTargetView) -> io::Result<Box<g::GlCtx>> {
        self.0.rent(|syms| (syms.1)(f, rtv))
    }

    fn cleanup(&self, ctx: Box<g::GlCtx>) {
        self.0.rent(|syms| (syms.2)(ctx))
    }

    // ought to have a join method that joins up with the driver thread...
}

pub fn load(path: &Path, comms: Box<DriverComms>) -> io::Result<Driver> {

    let lib = Library::new(path)?;
    {
        let version: Symbol<extern "C" fn() -> u32> = unsafe { lib.get(b"version\0") }?;
        let driver: Symbol<extern "C" fn(*mut DriverCallbacks)> = unsafe { lib.get(b"driver\0") }?;

        print!("loaded driver ");
        io::stdout().flush().ok().expect("flush1");
        println!("v{}", version());
        io::stdout().flush().ok().expect("flush2");

        let cbs = box DriverComms::into_callbacks(comms);
        driver(Box::into_raw(cbs));
    }

    rent_libloading::RentDriver::try_new(
        box lib,
        |lib| unsafe {
            let draw = lib.get(b"gl_draw\0")?;
            let setup = lib.get(b"gl_setup\0")?;
            let cleanup = lib.get(b"gl_cleanup\0")?;
            Ok((draw, setup, cleanup))
        })
        .map(Driver)
        .map_err(|err| err.0)
}

/// Generates function pointers and context for DriverCallbacks.
pub struct DriverComms {
    pub rx: mpsc::Receiver<Box<[u8]>>,
    pub tx: futures::sync::mpsc::UnboundedSender<Box<[u8]>>,
}

impl DriverComms {
    pub fn into_callbacks(comms: Box<DriverComms>) -> DriverCallbacks {
        DriverCallbacks {
            ctx: DriverCtx(Box::into_raw(comms) as *mut c_void),
            send_fn: driver_send,
            try_recv_fn: driver_try_recv,
            shutdown_fn: driver_shutdown,
        }
    }
}

extern fn driver_send(comms: *mut c_void, buf: *const u8, len: i32) -> i32 {
    let comms = unsafe { (comms as *mut DriverComms).as_mut().unwrap() };
    assert!(len > 0);
    assert!(!buf.is_null());

    let slice = unsafe { std::slice::from_raw_parts(buf, len as usize) };
    match comms.tx.send(slice.into()) {
        Ok(()) => 0,
        Err(_) => -1
    }
}

extern fn driver_try_recv(comms: *mut c_void, buf_out: *mut *mut u8) -> i32 {
    let comms = unsafe { (comms as *mut DriverComms).as_mut().unwrap() };
    assert!(!buf_out.is_null());

    match comms.rx.try_recv() {
        Ok(slice) => {
            let len = slice.len();
            assert!(len != 0);
            assert!(len <= std::i32::MAX as usize);
            unsafe {
                *buf_out = Box::into_raw(slice) as *mut u8;
            }
            len as i32
        }
        Err(mpsc::TryRecvError::Empty) => 0,
        Err(mpsc::TryRecvError::Disconnected) => -1
    }
}

extern fn driver_shutdown(comms: *mut c_void) {
    unsafe {
        drop(Box::from_raw(comms));
    }
}
