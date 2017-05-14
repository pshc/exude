#![feature(macro_reexport, nonzero)]

extern crate core;
#[macro_reexport(
    gfx_defines, gfx_format,
    gfx_pipeline, gfx_pipeline_base, gfx_pipeline_inner,
    gfx_impl_struct, gfx_impl_struct_meta,
    gfx_vertex_struct, gfx_vertex_struct_meta,
    gfx_constant_struct, gfx_constant_struct_meta,
)]
pub extern crate gfx;
#[cfg(feature = "gl")]
pub extern crate gfx_device_gl;
#[cfg(feature = "metal")]
pub extern crate gfx_device_metal;
pub extern crate gfx_text;
#[cfg(feature = "gl")]
pub extern crate gfx_window_glutin;
#[cfg(feature = "metal")]
pub extern crate gfx_window_metal;
#[cfg(feature = "gl")]
pub extern crate glutin;
#[cfg(feature = "metal")]
extern crate winit;

#[cfg(all(feature = "gl", not(feature = "metal")))]
pub use glutin::{ElementState, Event, MouseButton, MouseScrollDelta, ScanCode, VirtualKeyCode};
#[cfg(feature = "metal")]
pub use winit::{ElementState, Event, MouseButton, MouseScrollDelta, ScanCode, VirtualKeyCode};

use core::nonzero::NonZero;
use std::marker::PhantomData;

pub type ColorFormat = gfx::format::Rgba8;
pub type DepthFormat = gfx::format::Depth;

macro_rules! backend_items {
    ($backend:ident) => (
        use $backend;
        use core::nonzero::NonZero;
        use std::marker::PhantomData;
        use gfx;
        use super::{ColorFormat, DepthFormat, DriverRef};

        pub type Command = $backend::CommandBuffer;
        pub type Factory = $backend::Factory;
        pub type Res = $backend::Resources;

        pub type Encoder = gfx::Encoder<Res, Command>;
        pub type RenderTargetView = gfx::handle::RenderTargetView<Res, ColorFormat>;
        pub type DepthStencilView = gfx::handle::DepthStencilView<Res, DepthFormat>;

        pub type GlDrawFn = extern "C" fn(GfxRef, &mut Encoder);
        pub type GlUpdateFn = extern "C" fn(GfxRefMut, DriverRef, &mut Factory);
        pub type GlSetupFn = extern "C" fn(DriverRef, &mut Factory, RenderTargetView) -> Option<GfxBox>;
        pub type GlCleanupFn = extern "C" fn(GfxBox);

        /// Opaque user pointer passed into gfx-related functions.
        pub struct GfxBox(NonZero<*mut ()>);

        impl GfxBox {
            pub unsafe fn new(ptr: *mut ()) -> Option<Self> {
                if ptr.is_null() {
                    None
                } else {
                    Some(GfxBox(NonZero::new(ptr)))
                }
            }

            pub fn borrow<'a>(&'a self) -> GfxRef<'a> {
                let ptr = unsafe { NonZero::new(self.0.get() as *const ()) };
                GfxRef(ptr, PhantomData)
            }

            pub fn borrow_mut<'a>(&'a mut self) -> GfxRefMut<'a> {
                GfxRefMut(self.0, PhantomData)
            }

            pub fn consume(self) -> *mut () {
                self.0.get()
            }
        }

        /// Borrows GfxBox.
        #[derive(Clone, Copy)]
        pub struct GfxRef<'a>(pub NonZero<*const ()>, PhantomData<&'a ()>);

        /// Borrows GfxBox mutably.
        pub struct GfxRefMut<'a>(pub NonZero<*mut ()>, PhantomData<&'a mut ()>);
    )
}

#[cfg(feature = "gl")]
pub mod gl {
    backend_items!(gfx_device_gl);
}
#[cfg(all(feature = "gl", not(feature = "metal")))]
pub use gl::*;

#[cfg(feature = "metal")]
pub mod metal {
    backend_items!(gfx_device_metal);
}
#[cfg(all(not(feature = "gl"), feature = "metal"))]
pub use metal::*;

#[cfg(all(not(feature = "gl"), not(feature = "metal")))]
panic!("please enable at least one backend (e.g. cargo build --features=gl)");


/// Opaque user pointer passed into non-gfx-related functions.
pub struct DriverBox(NonZero<*mut ()>);

impl DriverBox {
    pub unsafe fn new(ptr: *mut ()) -> Option<Self> {
        if ptr.is_null() {
            None
        } else {
            Some(DriverBox(NonZero::new(ptr)))
        }
    }

    pub fn borrow<'a>(&'a self) -> DriverRef<'a> {
        let ptr = unsafe { NonZero::new(self.0.get() as *const ()) };
        DriverRef(ptr, PhantomData)
    }

    pub fn consume(self) -> *mut () {
        self.0.get()
    }
}

/// Borrows DriverBox.
#[derive(Clone, Copy)]
pub struct DriverRef<'a>(pub NonZero<*const ()>, PhantomData<&'a ()>);
