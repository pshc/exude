use std;
use std::io::{self, ErrorKind, Write};
use std::mem;
use std::path::Path;
use std::ptr;
use std::sync::mpsc;

use futures;
use libc::c_void;
use libloading::{Library, Symbol};

use driver_abi::{self, DriverCallbacks, DriverCtx, IoHandle};
use g;

rental! {
    mod rent_libloading {
        use driver_abi;
        use g;
        use libloading::{Library, Symbol};

        #[rental]
        pub struct RentDriver {
            lib: Box<Library>,
            syms: (Symbol<'lib, driver_abi::IoJoinFn>,
                   Symbol<'lib, g::GlDrawFn>,
                   Symbol<'lib, g::GlSetupFn>,
                   Symbol<'lib, g::GlCleanupFn>),
        }
    }
}

pub struct Driver {
    renter: rent_libloading::RentDriver,
    io_handle: IoHandle,
}

impl Driver {
    /// Must call this before dropping, or memory will be leaked.
    pub fn io_join(mut self) {
        let ref renter = self.renter;
        let handle = mem::replace(&mut self.io_handle, IoHandle(ptr::null_mut()));
        assert!(!handle.0.is_null());
        let cb_ptr = renter.rent(|syms| (syms.0)(handle));
        let mut callbacks: Box<DriverCallbacks> = unsafe { Box::from_raw(cb_ptr) };
        let comms = DriverComms::from_callbacks(&mut callbacks);
        drop(comms);
        drop(callbacks);
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        if !self.io_handle.0.is_null() {
            let _ = writeln!(io::stderr(), "WARNING: leaking driver thread");
            debug_assert!(false, "must call Driver::io_join");
        }
    }
}

impl g::GlInterface for Driver {
    fn draw(&self, ctx: &g::GlCtx, encoder: &mut g::Encoder) {
        self.renter.rent(|syms| (syms.1)(ctx, encoder))
    }

    fn setup(&self, f: &mut g::Factory, rtv: g::RenderTargetView) -> io::Result<Box<g::GlCtx>> {
        self.renter.rent(|syms| (syms.2)(f, rtv))
    }

    fn cleanup(&self, ctx: Box<g::GlCtx>) {
        self.renter.rent(|syms| (syms.3)(ctx))
    }
}

pub fn load(path: &Path, comms: Box<DriverComms>) -> io::Result<Driver> {

    let lib = Library::new(path)?;
    let io_handle;
    {
        let version: Symbol<driver_abi::VersionFn> = unsafe { lib.get(b"version\0") }?;
        let io_sym: Symbol<driver_abi::IoSpawnFn> = unsafe { lib.get(b"io_spawn\0") }?;

        print!("loaded driver ");
        io::stdout().flush().ok().expect("flush1");
        println!("v{}", version());
        io::stdout().flush().ok().expect("flush2");

        let cbs = Box::into_raw(box DriverComms::into_callbacks(comms));
        io_handle = io_sym(cbs);
        if io_handle.0.is_null() {
            // the thread could not be created.
            // error was dumped to stderr; shame we can't return it here...
            return Err(io::Error::new(ErrorKind::Other, "could not spawn driver IO"));
        }
    }

    rent_libloading::RentDriver::try_new(
        box lib,
        |lib| unsafe {
            let io_join = lib.get(b"io_join\0")?;
            let draw = lib.get(b"gl_draw\0")?;
            let setup = lib.get(b"gl_setup\0")?;
            let cleanup = lib.get(b"gl_cleanup\0")?;
            Ok((io_join, draw, setup, cleanup))
        })
        .map(|renter| Driver { renter, io_handle })
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
        }
    }

    fn from_callbacks(cbs: &mut DriverCallbacks) -> Box<Self> {
        let ptr = cbs.ctx.0 as *mut DriverComms;
        assert!(!ptr.is_null());
        cbs.ctx.0 = ptr::null_mut();
        unsafe { Box::from_raw(ptr) }
    }
}

extern "C" fn driver_send(ctx: DriverCtx, buf: *const u8, len: i32) -> i32 {
    let comms = unsafe { (ctx.0 as *mut DriverComms).as_mut().unwrap() };
    assert!(len > 0);
    assert!(!buf.is_null());

    let slice = unsafe { std::slice::from_raw_parts(buf, len as usize) };
    match comms.tx.send(slice.into()) {
        Ok(()) => 0,
        Err(_) => -1
    }
}

extern "C" fn driver_try_recv(ctx: DriverCtx, buf_out: *mut *mut u8) -> i32 {
    let comms = unsafe { (ctx.0 as *mut DriverComms).as_mut().unwrap() };
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
