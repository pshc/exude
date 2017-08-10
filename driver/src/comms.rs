use std::io::{self, Write};
use std::mem;
use std::ptr;

use errors::*;
use driver_abi::DriverCallbacks;
use proto::bincoded;
use proto::serde::{Deserialize, Serialize};

pub trait Pipe {
    fn send_on_chan<T: Serialize>(&self, Chan, &T) -> Result<()>;
    fn try_recv<T>(&self) -> Result<Option<T>>
    where
        for<'de> T: Deserialize<'de>;

    fn send<T: Serialize>(&self, msg: &T) -> Result<()> {
        self.send_on_chan(Chan::Server, msg)
    }
}

/// Whether to send to the server or to the client loader (control).
#[derive(Clone, Copy, Debug)]
pub enum Chan {
    Server,
    Control,
}

/// Safe high-level wrapper for DriverCallbacks.
pub struct Wrapper(*mut DriverCallbacks);

impl Wrapper {
    pub fn new(cbs: *mut DriverCallbacks) -> Self {
        assert!(!cbs.is_null());
        Wrapper(cbs)
    }

    /// Must be called or memory will leak.
    pub fn consume(mut self) -> *mut DriverCallbacks {
        mem::replace(&mut self.0, ptr::null_mut())
    }
}

impl Pipe for Wrapper {
    fn send_on_chan<T: Serialize>(&self, chan: Chan, msg: &T) -> Result<()> {
        let cbs = unsafe { &*self.0 };

        let len = bincoded::serialized_size(msg)? as i32;
        let packet = (cbs.alloc_fn)(cbs.ctx, len);
        assert!(!packet.is_null());
        let mut packet_ref = unsafe { ::std::slice::from_raw_parts_mut(packet, len as usize) };
        match bincoded::bincode::serialize_into(&mut packet_ref, msg, bincoded::bincode::Infinite) {
            Ok(()) => {
                let send = match chan {
                    Chan::Server => cbs.send_fn,
                    Chan::Control => cbs.control_write_fn,
                };
                let code = send(cbs.ctx, packet, len);
                ensure!(code >= 0, ErrorKind::BrokenComms);
                Ok(())
            }
            Err(e) => {
                (cbs.free_fn)(cbs.ctx, packet, len);
                Err(e.into())
            }
        }
    }

    fn try_recv<T>(&self) -> Result<Option<T>>
    where
        for<'de> T: Deserialize<'de>,
    {
        let cbs = unsafe { &*self.0 };
        let mut packet = ptr::null_mut();
        let len = (cbs.try_recv_fn)(cbs.ctx, &mut packet);
        if len > 0 {
            let slice = unsafe { ::std::slice::from_raw_parts(packet, len as usize) };
            let result = bincoded::deserialize_exact(slice).chain_err(|| "couldn't decode message");
            (cbs.free_fn)(cbs.ctx, packet, len);
            result.map(Some)
        } else if len == 0 {
            Ok(None)
        } else {
            bail!(ErrorKind::BrokenComms);
        }
    }
}

impl Drop for Wrapper {
    fn drop(&mut self) {
        if !self.0.is_null() {
            let _ = writeln!(io::stderr(), "WARNING: leaking driver callbacks");
            debug_assert!(false, "must call Wrapper::consume()");
        }
    }
}
