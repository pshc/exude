//! Shared interface between the loader and driver.

use std::io;

use g::gfx;
use g::gfx_device_gl;
use libc::c_void;

#[derive(Debug, Deserialize, Serialize)]
pub enum UpRequest {
    Ping(u32),
    Bye,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum DownResponse {
    Pong(u32),
}

// future work: macro for generating multiple backends (vulkan, ...)
pub type Res = gfx_device_gl::Resources;
pub type Command = gfx_device_gl::CommandBuffer;

pub type ColorFormat = gfx::format::Rgba8;

pub type GlDrawFn = extern fn(data: &DrawGL, encoder: &mut gfx::Encoder<Res, Command>);
pub type GlSetupFn = extern fn(&mut gfx_device_gl::Factory,
                               gfx::handle::RenderTargetView<gfx_device_gl::Resources, ColorFormat>)
                               -> io::Result<Box<DrawGL>>;
pub type GlCleanupFn = extern fn(data: Box<DrawGL>);

pub trait DrawGL {
    fn draw(&self, &mut gfx::Encoder<Res, Command>);
}

pub struct DriverCtx(pub *mut c_void);
unsafe impl Send for DriverCtx {}

/// For transmitting messages between driver and client core.
/// Uses C ABI in an attempt at interface stability.
#[repr(C)]
pub struct DriverEnv {
    /// Must be passed into all below functions.
    pub ctx: DriverCtx,

    /// Passed bytes will be copied to an internal buffer.
    /// On error, returns a negative value.
    pub send_fn: extern fn(*mut c_void, buf: *const u8, len: i32) -> i32,

    /// Attempt to receive a message, non-blocking.
    /// On message, writes the pointer to a new allocated buffer, and returns its length.
    /// If no messages are pending, returns zero.
    /// On error, returns a negative value.
    pub try_recv_fn: extern fn(*mut c_void, buf_out: *mut *mut u8) -> i32,

    /// Closes communication channels and frees memory.
    pub shutdown_fn: extern fn(*mut c_void),
}

pub mod bincoded {
    #![allow(dead_code)] // TEMP

    use std::io::{self, ErrorKind};
    use std::marker::PhantomData;

    use bincode;
    use serde::{Deserialize, Serialize};

    /// Holds the result of `bincode::serialize`.
    #[derive(Clone)]
    pub struct Bincoded<T> {
        vec: Vec<u8>,
        _phantom: PhantomData<T>,
    }

    pub static BINCODED_MAX: u64 = 0xffff;

    fn to_io_err(err: bincode::Error) -> io::Error {
        match *err {
            bincode::ErrorKind::IoError(io) => io,
            e => io::Error::new(ErrorKind::Other, e)
        }
    }

    impl<T> Bincoded<T> {
        /// Returns the number of serialized bytes stored. Does not include length header.
        pub fn len(&self) -> usize {
            self.vec.len()
        }

        /// Not intended for general use; this is for low-level use.
        /// Precondition: `vec` must have been encoded with the same T.
        pub unsafe fn from_vec(vec: Vec<u8>) -> Self {
            Bincoded {vec: vec, _phantom: PhantomData}
        }
    }

    impl<T: Serialize> Bincoded<T> {
        /// Serializes `value`, storing the serialized bytes in `self`.
        pub fn new(value: &T) -> io::Result<Self> {
            let size_limit = bincode::Bounded(BINCODED_MAX);
            Ok(Bincoded {
                vec: bincode::serialize(value, size_limit).map_err(to_io_err)?,
                _phantom: PhantomData,
            })
        }
    }

    pub fn deserialize_exact<R: AsRef<[u8]>, T: Deserialize>(slice: R) -> io::Result<T> {
        let slice = slice.as_ref();
        let len = slice.len() as u64;
        let ref mut cursor = io::Cursor::new(slice);
        let result = bincode::deserialize_from(cursor, bincode::Infinite).map_err(to_io_err)?;

        // ensure the deserializer consumed every last byte
        if cursor.position() == len {
            Ok(result)
        } else {
            let msg = format!("extra bytes ({})", len - cursor.position());
            let io = io::Error::new(ErrorKind::InvalidData, msg);
            Err(io)
        }

    }

    impl<T: Deserialize> Bincoded<T> {
        /// Deserialize the contained bytes.
        pub fn deserialize(&self) -> io::Result<T> {
            deserialize_exact(self)
        }
    }

    impl<T> AsRef<[u8]> for Bincoded<T> {
        fn as_ref(&self) -> &[u8] {
            self.vec.as_ref()
        }
    }

    impl<T> Into<Vec<u8>> for Bincoded<T> {
        fn into(self) -> Vec<u8> {
            self.vec
        }
    }

    #[test]
    fn roundtrip() {
        let orig = (42, format!("hello"));
        let coded = Bincoded::new(&orig).unwrap().deserialize().unwrap();
        assert_eq!(orig, coded);
    }

    #[test]
    fn too_short() {
        let short: Bincoded<u32> = Bincoded {vec: vec![1, 2, 3], _phantom: PhantomData};
        let err = short.deserialize().unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnexpectedEof);
    }

    #[test]
    fn too_long() {
        use std::iter;
        use std::error::Error;
        let bytes: Vec<u8> = iter::repeat(0).take(17).collect();
        let long: Bincoded<(u64, u64)> = Bincoded {vec: bytes, _phantom: PhantomData};
        let err = long.deserialize().unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
        assert_eq!(err.description(), "extra bytes (1)");
    }
}
