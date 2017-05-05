use std::io::{self, ErrorKind, Write};
use std::mem;
use std::ptr;

use driver_abi::DriverCallbacks;
use proto::bincoded::{self, Bincoded};
use proto::serde::{Deserialize, Serialize};

pub trait Pipe {
    fn send<T: Serialize>(&self, &T) -> io::Result<()>;
    fn try_recv<T>(&self) -> io::Result<Option<T>>
    where
        for<'de> T: Deserialize<'de>;
}

/// Safe high-level wrapper for DriverCallbacks.
pub struct Wrapper(*mut DriverCallbacks);
unsafe impl Send for Wrapper {}

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
    fn send<T: Serialize>(&self, msg: &T) -> io::Result<()> {
        let cbs = unsafe { &*self.0 };

        // so many copies... ugh!
        let bin = Bincoded::new(msg)?;
        let vec: Vec<u8> = bin.into();
        assert!(vec.len() <= ::std::i32::MAX as usize);
        if (cbs.send_fn)(cbs.ctx, vec.as_ptr(), vec.len() as i32) >= 0 {
            Ok(())
        } else {
            Err(io::Error::new(ErrorKind::BrokenPipe, "send: pipe broken"))
        }
    }

    fn try_recv<T>(&self) -> io::Result<Option<T>>
    where
        for<'de> T: Deserialize<'de>,
    {
        let cbs = unsafe { &*self.0 };
        let mut buf_ptr = ptr::null_mut();
        let len = (cbs.try_recv_fn)(cbs.ctx, &mut buf_ptr);
        if len > 0 {
            let slice = unsafe { ::std::slice::from_raw_parts(buf_ptr, len as usize) };
            let result = bincoded::deserialize_exact(slice);
            unsafe { drop(Box::from_raw(buf_ptr)) }
            result.map(Some)
        } else if len == 0 {
            Ok(None)
        } else {
            let err = io::Error::new(ErrorKind::BrokenPipe, "try_recv: pipe broken");
            Err(err)
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