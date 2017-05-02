use std;
use std::io::{self, Write};
use std::path::Path;
use std::sync::mpsc;

use futures;
use libc::c_void;
use libloading::{self, Library, Symbol};

use env::{DriverCtx, DriverEnv};
use g;

pub struct Api<'lib> {
    s_driver: Symbol<'lib, extern fn(*mut DriverEnv)>,
    s_version: Symbol<'lib, extern fn() -> u32>,
}

impl<'lib> Api<'lib> {
    pub unsafe fn new(lib: &'lib Library) -> libloading::Result<Self> {
        // hack... make sure those symbols will load later
        let _ = lib.get::<g::GlDrawFn>(b"gl_draw\0")?;
        let _ = lib.get::<g::GlSetupFn>(b"gl_setup\0")?;
        let _ = lib.get::<g::GlCleanupFn>(b"gl_cleanup\0")?;

        Ok(Api {
            s_driver: lib.get(b"driver\0")?,
            s_version: lib.get(b"version\0")?,
        })
    }

    pub fn driver(&self, env: Box<DriverEnv>) {
        (*self.s_driver)(Box::into_raw(env))
    }

    pub fn version(&self) -> u32 {
        (*self.s_version)()
    }
}

// TODO use `rental` to store library in here with the symbols!
// also figure out a better name
pub struct Driver(Library);

impl Driver {
    pub fn gl_draw<'lib>(&'lib self) -> Symbol<'lib, g::GlDrawFn> {
        unsafe { self.0.get(b"gl_draw\0").unwrap() }
    }

    pub fn gl_setup<'lib>(&'lib self) -> Symbol<'lib, g::GlSetupFn> {
        unsafe { self.0.get(b"gl_setup\0").unwrap() }
    }

    pub fn gl_cleanup<'lib>(&'lib self) -> Symbol<'lib, g::GlCleanupFn> {
        unsafe { self.0.get(b"gl_cleanup\0").unwrap() }
    }

    // ought to have a join method that joins up with the driver thread...
}

pub fn load(path: &Path, comms: Box<DriverComms>) -> libloading::Result<Driver> {

    let lib = Library::new(path)?;
    {
        let api = unsafe { Api::new(&lib)? };

        print!("loaded driver ");
        io::stdout().flush().ok().expect("flush1");
        println!("v{}", api.version());
        io::stdout().flush().ok().expect("flush2");

        let env = DriverComms::into_env(comms);
        api.driver(box env);
    }
    return Ok(Driver(lib))

    // crashes here if any driver code is still being run (because Library is dropped)
}

/// Generates function pointers and context for DriverEnv.
pub struct DriverComms {
    pub rx: mpsc::Receiver<Box<[u8]>>,
    pub tx: futures::sync::mpsc::UnboundedSender<Box<[u8]>>,
}

impl DriverComms {
    pub fn into_env(comms: Box<DriverComms>) -> DriverEnv {
        DriverEnv {
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
