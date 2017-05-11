//! Used by oneshot.

#![feature(box_syntax)]
#![recursion_limit = "1024"]

#[macro_use]
extern crate error_chain;
extern crate futures;
extern crate g;
extern crate proto;
extern crate tokio_io;

#[path="../../server/src/common.rs"]
pub mod common;
pub mod errors;
pub mod net;
pub mod render_loop;

pub use errors::*;
