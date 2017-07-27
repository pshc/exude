//! Used by oneshot.

#![feature(box_syntax)]
#![recursion_limit = "1024"]

#[macro_use]
extern crate error_chain;
extern crate futures;
extern crate g;
// oneshot doesn't actually need hyper, at this point
// ... but we have to import it due to errors' Hyper case.
// maybe we could use a cfg attr to skip this?
extern crate hyper;
extern crate proto;
extern crate tokio_core;
extern crate tokio_io;
extern crate tokio_timer;

#[path="../../server/src/common.rs"]
pub mod common;
pub mod errors;
pub mod net;
pub mod render_loop;

pub use errors::*;
