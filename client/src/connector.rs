use std;
use std::collections::HashMap;
use std::io::{self, ErrorKind, Write};
use std::mem;
use std::path::Path;
use std::ptr;
use std::sync::mpsc;

use futures::sync::mpsc::UnboundedSender;
use libloading::{Library, Symbol};

use driver_abi::{self, CallbackCtx, DriverCallbacks};
use g;
use proto::Bytes;

rental! {
    mod rent_libloading {
        use driver_abi;
        use g;
        use libloading::{Library, Symbol};

        #[rental]
        pub struct RentDriver {
            lib: Box<Library>,
            syms: (Symbol<'lib, driver_abi::TeardownFn>,
                   Symbol<'lib, g::GlDrawFn>,
                   Symbol<'lib, g::GlUpdateFn>,
                   Symbol<'lib, g::GlSetupFn>,
                   Symbol<'lib, g::GlCleanupFn>),
        }
    }
}

pub struct Driver {
    renter: rent_libloading::RentDriver,
    handle: Option<g::DriverBox>,
}

impl Driver {
    /// Must call this before dropping, or memory will be leaked.
    pub fn join(mut self) {
        let ref renter = self.renter;
        let handle = mem::replace(&mut self.handle, None).expect("join: null handle");
        let cb_ptr = renter.rent(|syms| (syms.0)(handle));
        assert!(!cb_ptr.is_null());
        println!("Final driver teardown.");
        let mut callbacks: Box<DriverCallbacks> = unsafe { Box::from_raw(cb_ptr) };
        let comms = DriverComms::from_callbacks(&mut callbacks);
        drop(comms);
        drop(callbacks);
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        if self.handle.is_some() {
            let _ = writeln!(io::stderr(), "WARNING: leaking driver state");
            debug_assert!(false, "must call Driver::join");
        }
    }
}

impl Driver {
    pub fn draw(&self, ctx: g::GfxRef, encoder: &mut g::Encoder) {
        self.renter.rent(|syms| (syms.1)(ctx, encoder))
    }

    pub fn update(&mut self, ctx: g::GfxRefMut, factory: &mut g::Factory) {
        let handle = self.handle.as_mut().expect("update: null handle");
        self.renter
            .rent(|syms| (syms.2)(ctx, handle.borrow_mut(), factory))
    }

    pub fn gfx_setup(&self, f: &mut g::Factory, v: g::RenderTargetView) -> Option<g::GfxBox> {
        let handle = self.handle.as_ref().expect("setup: null handle");
        self.renter.rent(|syms| (syms.3)(handle.borrow(), f, v))
    }

    pub fn gfx_cleanup(&self, ctx: g::GfxBox) {
        self.renter.rent(|syms| (syms.4)(ctx))
    }
}

pub fn load(path: &Path, comms: Box<DriverComms>) -> io::Result<Driver> {

    let lib = Library::new(path)?;
    let handle;
    {
        let version: Symbol<driver_abi::VersionFn> = unsafe { lib.get(b"version\0") }?;
        let setup: Symbol<driver_abi::SetupFn> = unsafe { lib.get(b"setup\0") }?;

        print!("loaded driver ");
        io::stdout().flush().ok().expect("flush1");
        println!("v{}", version());
        io::stdout().flush().ok().expect("flush2");

        let cbs = Box::into_raw(box DriverComms::into_callbacks(comms));
        handle = setup(cbs);
        if handle.is_none() {
            // error was dumped to stderr; shame we can't return it here...
            let err = io::Error::new(ErrorKind::Other, "could not setup driver");
            return Err(err);
        }
    }

    rent_libloading::RentDriver::try_new(
        box lib, |lib| unsafe {
            let teardown = lib.get(b"teardown\0")?;
            let draw = lib.get(b"gl_draw\0")?;
            let update = lib.get(b"gl_update\0")?;
            let setup = lib.get(b"gl_setup\0")?;
            let cleanup = lib.get(b"gl_cleanup\0")?;
            Ok((teardown, draw, update, setup, cleanup))
        }
    )
            .map(|renter| Driver { renter, handle })
            .map_err(|err| err.0)
}

/// Generates function pointers and context for DriverCallbacks.
pub struct DriverComms {
    pub rx: mpsc::Receiver<Bytes>,
    pub tx: UnboundedSender<Bytes>,
    pub control_tx: mpsc::Sender<Bytes>,
    packets: HashMap<usize, (usize, usize)>,
}

/// If there are more packets than this at once, we are likely leaking (or backed up...?)
const MAX_PACKETS: usize = 32;

impl DriverComms {
    pub fn new(
        rx: mpsc::Receiver<Bytes>,
        tx: UnboundedSender<Bytes>,
        control_tx: mpsc::Sender<Bytes>,
    ) -> Self {
        DriverComms { rx, tx, control_tx, packets: HashMap::with_capacity(MAX_PACKETS) }
    }

    pub fn into_callbacks(comms: Box<DriverComms>) -> DriverCallbacks {
        DriverCallbacks {
            ctx: CallbackCtx(Box::into_raw(comms) as *mut ()),
            send_fn: driver_send,
            control_write_fn: control_write,
            try_recv_fn: driver_try_recv,
            alloc_fn: driver_alloc,
            free_fn: driver_free,
        }
    }

    fn from_callbacks(cbs: &mut DriverCallbacks) -> Box<Self> {
        let ptr = cbs.ctx.0 as *mut DriverComms;
        assert!(!ptr.is_null());
        cbs.ctx.0 = ptr::null_mut();
        unsafe { Box::from_raw(ptr) }
    }

    fn with_ctx<T, F: FnOnce(&mut Self) -> T>(ctx: CallbackCtx, f: F) -> T {
        let comms = unsafe { (ctx.0 as *mut DriverComms).as_mut() }.expect("null ctx");
        f(comms)
    }

    /// Must call `vec_from_packet` with the result, or memory will leak.
    fn packet_from_vec(&mut self, mut vec: Vec<u8>) -> *mut u8 {
        if self.packets.len() == MAX_PACKETS {
            println!("warning: too many packets concurrently allocated; possible memory leak");
        }
        let len = vec.len();
        let cap = vec.capacity();
        let ptr = vec.as_mut_ptr();
        assert!(!ptr.is_null());
        // remember this packet's details
        self.packets.insert(ptr as usize, (len, cap));
        // and return it as a pointer
        mem::forget(vec);
        ptr
    }

    unsafe fn vec_from_packet(&mut self, packet: *mut u8, len: usize) -> Vec<u8> {
        let (stored_len, cap) = self.packets.remove(&(packet as usize)).expect("free invalid");
        assert_eq!(len, stored_len);
        assert!(cap >= len);
        Vec::from_raw_parts(packet, len, cap)
    }
}

impl Drop for DriverComms {
    fn drop(&mut self) {
        if !self.packets.is_empty() {
            println!("comms: leaked {} packet(s)", self.packets.len());
        }
    }
}

/// Called from the driver to allocate a packet for sending.
extern "C" fn driver_alloc(ctx: CallbackCtx, len: i32) -> *mut u8 {
    assert!(len > 0);
    DriverComms::with_ctx(ctx, |comms| {
        comms.packet_from_vec(vec![0u8; len as usize])
    })
}

/// Called from the driver to free a packet it received.
extern "C" fn driver_free(ctx: CallbackCtx, ptr: *mut u8, len: i32) {
    assert!(!ptr.is_null());
    assert!(len > 0);
    DriverComms::with_ctx(ctx, |comms| {
        let vec = unsafe { comms.vec_from_packet(ptr, len as usize) };
        drop(vec);
    })
}

/// Called from driver to send messages to the server, c/o us (the client).
extern "C" fn driver_send(ctx: CallbackCtx, packet: *mut u8, len: i32) -> i32 {
    assert!(len > 0);
    assert!(!packet.is_null());

    DriverComms::with_ctx(ctx, |comms| {
        let vec = unsafe { comms.vec_from_packet(packet, len as usize) };
        match comms.tx.send(vec.into()) {
            Ok(()) => 0,
            Err(_) => -1,
        }
    })
}

/// Called from driver to issue control messages to us.
extern "C" fn control_write(ctx: CallbackCtx, packet: *mut u8, len: i32) -> i32 {
    assert!(len > 0);
    assert!(!packet.is_null());

    DriverComms::with_ctx(ctx, |comms| {
        let vec = unsafe { comms.vec_from_packet(packet, len as usize) };
        match comms.control_tx.send(vec.into()) {
            Ok(()) => 0,
            Err(_) => -1,
        }
    })
}

/// Called from driver to check for messages inbound from the server, c/o us.
extern "C" fn driver_try_recv(ctx: CallbackCtx, packet_out: *mut *mut u8) -> i32 {
    assert!(!packet_out.is_null());

    DriverComms::with_ctx(ctx, |comms| {
        match comms.rx.try_recv() {
            Ok(bytes) => {
                // ideally we would transform `bytes` into a `vec` here, rather than copy:
                // https://github.com/carllerche/bytes/issues/86
                let vec = bytes.to_vec();

                let len = vec.len();
                assert!(len != 0);
                assert!(len <= std::i32::MAX as usize);

                unsafe {
                    *packet_out = comms.packet_from_vec(vec);
                }
                len as i32
            }
            Err(mpsc::TryRecvError::Empty) => 0,
            Err(mpsc::TryRecvError::Disconnected) => -1,
        }
    })
}
