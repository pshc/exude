use std::io::{self, ErrorKind};
use std::ptr;

use serde::{Deserialize, Serialize};

use env::DriverEnv;
use proto::bincoded::{self, Bincoded};

pub struct EnvWrapper(Box<DriverEnv>);

impl EnvWrapper {
    pub fn wrap(env: *mut DriverEnv) -> Self {
        EnvWrapper(unsafe { Box::from_raw(env) })
    }

    pub fn send<T: Serialize>(&self, msg: &T) -> io::Result<()> {
        // so many copies... ugh!
        let bin = Bincoded::new(msg)?;
        let vec: Vec<u8> = bin.into();
        assert!(vec.len() <= ::std::i32::MAX as usize);
        if (self.0.send_fn)(self.0.ctx.0, vec.as_ptr(), vec.len() as i32) >= 0 {
            Ok(())
        } else {
            Err(io::Error::new(ErrorKind::BrokenPipe, "send: pipe broken"))
        }
    }

    pub fn try_recv<T: Deserialize>(&self) -> io::Result<Option<T>> {
        let mut buf_ptr = ptr::null_mut();
        let len = (self.0.try_recv_fn)(self.0.ctx.0, &mut buf_ptr);
        if len > 0 {
            let slice = unsafe { ::std::slice::from_raw_parts(buf_ptr, len as usize) };
            let result = bincoded::deserialize_exact(slice);
            unsafe {
                drop(Box::from_raw(buf_ptr))
            }
            result.map(Some)
        } else if len == 0 {
            Ok(None)
        } else {
            Err(io::Error::new(ErrorKind::BrokenPipe, "try_recv: pipe broken"))
        }
    }
}

impl Drop for EnvWrapper {
    fn drop(&mut self) {
        (self.0.shutdown_fn)(self.0.ctx.0)
    }
}
