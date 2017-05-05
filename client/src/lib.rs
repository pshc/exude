//! Used by oneshot.

#![feature(box_syntax)]

extern crate futures;
extern crate g;
extern crate proto;
extern crate tokio_io;

#[path="../../server/src/common.rs"]
pub mod common;
pub mod net;
pub mod render_loop;
