//! Wayland Interfaces we care about
//!
//! I only bothered implementing the interfaces we actually use. The initial implementation was
//! done with an ad-hoc wayland scanner, but then I later refined it to make it more specific to
//! what `swww-daemon` needs. Specifically, we use a lot of the globals in `super::globals` to
//! simplify the code.

use super::{
    globals,
    wire::{WaylandPayload, WireMsg, WireMsgBuilder, WlFixed},
    ObjectId,
};

///core global object
///
///The core global object.  This is a special singleton object.  It
///is used for internal Wayland protocol features.
pub mod wl_display {
    use super::*;

    pub trait EvHandler {
        ///fatal error event
        ///
        ///The error event is sent out when a fatal (non-recoverable)
        ///error has occurred.  The object_id argument is the object
        ///where the error occurred, most often in response to a request
        ///to that object.  The code identifies the error and is defined
        ///by the object interface.  As such, each interface defines its
        ///own set of error codes.  The message is a brief description
        ///of the error, for (debugging) convenience.
        fn error(&mut self, object_id: ObjectId, code: u32, message: &str) {
            let interface = match object_id {
                globals::WL_DISPLAY => "wl_display",
                globals::WL_REGISTRY => "wl_registry",
                globals::WL_COMPOSITOR => "wl_compositor",
                globals::WL_SHM => "wl_shm",
                globals::WP_VIEWPORTER => "wp_viewporter",
                globals::ZWLR_LAYER_SHELL_V1 => "zwlr_layer_shell_v1",
                other => match super::super::globals::object_type_get(other) {
                    Some(super::super::WlDynObj::Output) => "wl_output",
                    Some(super::super::WlDynObj::Surface) => "wl_surface",
                    Some(super::super::WlDynObj::Region) => "wl_region",
                    Some(super::super::WlDynObj::LayerSurface) => "zwlr_layer_surface_v1",
                    Some(super::super::WlDynObj::Buffer) => "wl_buffer",
                    Some(super::super::WlDynObj::ShmPool) => "wl_shm_pool",
                    Some(super::super::WlDynObj::Callback) => "wl_callback",
                    Some(super::super::WlDynObj::Viewport) => "wl_viewport",
                    Some(super::super::WlDynObj::FractionalScale) => "wp_fractional_scale_v1",
                    None => "???",
                },
            };

            panic!("Protocol error on interface {interface}. Code {code}: {message}");
        }
        ///acknowledge object ID deletion
        ///
        ///This event is used internally by the object ID management
        ///logic. When a client deletes an object that it had created,
        ///the server will send this event to acknowledge that it has
        ///seen the delete request. When the client receives this event,
        ///it will know that it can safely reuse the object ID.
        fn delete_id(&mut self, id: u32);
    }

    pub fn event<T: EvHandler>(state: &mut T, mut wire_msg: WireMsg, payload: WaylandPayload) {
        match wire_msg.op() {
            0 => {
                let object_id = wire_msg.next_object(&payload).unwrap();
                let code = wire_msg.next_u32(&payload);
                let message = wire_msg.next_string(&payload);
                state.error(object_id, code, message);
            }
            1 => state.delete_id(wire_msg.next_u32(&payload)),
            e => log::error!("unrecognized event opcode: {e} for interface wl_display"),
        }
    }

    ///Requests for this interface
    pub mod req {
        use super::*;
        ///asynchronous roundtrip
        ///
        ///The sync request asks the server to emit the 'done' event
        ///on the returned wl_callback object.  Since requests are
        ///handled in-order and events are delivered in-order, this can
        ///be used as a barrier to ensure all previous requests and the
        ///resulting events have been handled.
        ///
        ///The object returned by this request will be destroyed by the
        ///compositor after the callback is fired and as such the client must not
        ///attempt to use it after that point.
        ///
        ///The callback_data passed in the callback is the event serial.
        pub fn sync(callback: ObjectId) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(globals::WL_DISPLAY, 0);
            wire_msg_builder.add_new_specified_id(callback);
            wire_msg_builder.send()
        }
        ///get global registry object
        ///
        ///This request creates a registry object that allows the client
        ///to list and bind the global objects available from the
        ///compositor.
        ///
        ///It should be noted that the server side resources consumed in
        ///response to a get_registry request can only be released when the
        ///client disconnects, not when the client side proxy is destroyed.
        ///Therefore, clients should invoke get_registry as infrequently as
        ///possible to avoid wasting memory.
        pub fn get_registry() -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(globals::WL_DISPLAY, 1);
            wire_msg_builder.add_new_specified_id(globals::WL_REGISTRY);
            wire_msg_builder.send()
        }
    }
    ///global error values
    ///
    ///These errors are global and can be emitted in response to any
    ///server request.
    pub mod error {
        ///server couldn't find object
        pub const INVALID_OBJECT: u32 = 0u32;
        ///method doesn't exist on the specified interface or malformed request
        pub const INVALID_METHOD: u32 = 1u32;
        ///server is out of memory
        pub const NO_MEMORY: u32 = 2u32;
        ///implementation error in compositor
        pub const IMPLEMENTATION: u32 = 3u32;
    }
}
///global registry object
///
///The singleton global registry object.  The server has a number of
///global objects that are available to all clients.  These objects
///typically represent an actual object in the server (for example,
///an input device) or they are singleton objects that provide
///extension functionality.
///
///When a client creates a registry object, the registry object
///will emit a global event for each global currently in the
///registry.  Globals come and go as a result of device or
///monitor hotplugs, reconfiguration or other events, and the
///registry will send out global and global_remove events to
///keep the client up to date with the changes.  To mark the end
///of the initial burst of events, the client can use the
///wl_display.sync request immediately after calling
///wl_display.get_registry.
///
///A client can bind to a global object by using the bind
///request.  This creates a client-side handle that lets the object
///emit events to the client and lets the client invoke requests on
///the object.
pub mod wl_registry {
    use super::*;

    pub trait EvHandler {
        ///announce global object
        ///
        ///Notify the client of global objects.
        ///
        ///The event notifies the client that a global object with
        ///the given name is now available, and it implements the
        ///given version of the given interface.
        fn global(&mut self, name: u32, interface: &str, version: u32);
        ///announce removal of global object
        ///
        ///Notify the client of removed global objects.
        ///
        ///This event notifies the client that the global identified
        ///by name is no longer available.  If the client bound to
        ///the global using the bind request, the client should now
        ///destroy that object.
        ///
        ///The object remains valid and requests to the object will be
        ///ignored until the client destroys it, to avoid races between
        ///the global going away and a client sending a request to it.
        fn global_remove(&mut self, name: u32);
    }

    pub fn event<T: EvHandler>(state: &mut T, mut wire_msg: WireMsg, payload: WaylandPayload) {
        match wire_msg.op() {
            0 => {
                let name = wire_msg.next_u32(&payload);
                let interface = wire_msg.next_string(&payload);
                let version = wire_msg.next_u32(&payload);
                state.global(name, interface, version);
            }
            1 => state.global_remove(wire_msg.next_u32(&payload)),
            e => log::error!("unrecognized event opcode: {e} for interface wl_registry"),
        }
    }

    ///Requests for this interface
    pub mod req {
        use super::*;
        ///bind an object to the display
        ///
        ///Binds a new, client-created object to the server using the
        ///specified name as the identifier.
        pub fn bind(
            name: u32,
            id: ObjectId,
            id_interface: &str,
            id_version: u32,
        ) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(globals::WL_REGISTRY, 0);
            wire_msg_builder.add_u32(name);
            wire_msg_builder.add_new_unspecified_id(id, id_interface, id_version);
            wire_msg_builder.send()
        }
    }
}
///callback object
///
///Clients can handle the 'done' event to get notified when
///the related request is done.
///
///Note, because wl_callback objects are created from multiple independent
///factory interfaces, the wl_callback interface is frozen at version 1.
pub mod wl_callback {
    use super::*;

    pub trait EvHandler {
        ///done event
        ///
        ///Notify the client when the related request is done.
        ///
        ///THIS IS A DESTRUCTOR
        fn done(&mut self, sender_id: ObjectId, callback_data: u32);
    }

    ///Requests for this interface
    pub mod req {}

    pub fn event<T: EvHandler>(state: &mut T, mut wire_msg: WireMsg, payload: WaylandPayload) {
        match wire_msg.op() {
            0 => {
                let callback_data = wire_msg.next_u32(&payload);
                state.done(wire_msg.sender_id(), callback_data);
            }
            e => log::error!("unrecognized event opcode: {e} for interface wl_callback"),
        }
    }
}
///the compositor singleton
///
///A compositor.  This object is a singleton global.  The
///compositor is in charge of combining the contents of multiple
///surfaces into one displayable output.
pub mod wl_compositor {
    use super::*;

    ///Events for this interface
    pub mod ev {}
    ///Requests for this interface
    pub mod req {
        use super::*;
        ///create new surface
        ///
        ///Ask the compositor to create a new surface.
        pub fn create_surface(id: ObjectId) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(globals::WL_COMPOSITOR, 0);
            wire_msg_builder.add_new_specified_id(id);
            wire_msg_builder.send()
        }
        ///create new region
        ///
        ///Ask the compositor to create a new region.
        pub fn create_region(id: ObjectId) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(globals::WL_COMPOSITOR, 1);
            wire_msg_builder.add_new_specified_id(id);
            wire_msg_builder.send()
        }
    }
}
///a shared memory pool
///
///The wl_shm_pool object encapsulates a piece of memory shared
///between the compositor and client.  Through the wl_shm_pool
///object, the client can allocate shared memory wl_buffer objects.
///All objects created through the same pool share the same
///underlying mapped memory. Reusing the mapped memory avoids the
///setup/teardown overhead and is useful when interactively resizing
///a surface or for many small buffers.
pub mod wl_shm_pool {
    use super::*;

    ///Events for this interface
    pub mod ev {}
    ///Requests for this interface
    pub mod req {
        use super::*;
        ///create a buffer from the pool
        ///
        ///Create a wl_buffer object from the pool.
        ///
        ///The buffer is created offset bytes into the pool and has
        ///width and height as specified.  The stride argument specifies
        ///the number of bytes from the beginning of one row to the beginning
        ///of the next.  The format is the pixel format of the buffer and
        ///must be one of those advertised through the wl_shm.format event.
        ///
        ///A buffer will keep a reference to the pool it was created from
        ///so it is valid to destroy the pool immediately after creating
        ///a buffer from it.
        #[allow(clippy::too_many_arguments)]
        pub fn create_buffer(
            sender_id: ObjectId,
            id: ObjectId,
            offset: i32,
            width: i32,
            height: i32,
            stride: i32,
            format: u32,
        ) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 0);
            wire_msg_builder.add_new_specified_id(id);
            wire_msg_builder.add_i32(offset);
            wire_msg_builder.add_i32(width);
            wire_msg_builder.add_i32(height);
            wire_msg_builder.add_i32(stride);
            wire_msg_builder.add_u32(format);
            wire_msg_builder.send()
        }

        ///destroy the pool
        ///
        ///Destroy the shared memory pool.
        ///
        ///The mmapped memory will be released when all
        ///buffers that have been created from this pool
        ///are gone.
        ///
        ///THIS IS A DESTRUCTOR
        pub fn destroy(sender_id: ObjectId) -> rustix::io::Result<()> {
            let wire_msg_builder = WireMsgBuilder::new(sender_id, 1);
            wire_msg_builder.send()
        }
        ///change the size of the pool mapping
        ///
        ///This request will cause the server to remap the backing memory
        ///for the pool from the file descriptor passed when the pool was
        ///created, but using the new size.  This request can only be
        ///used to make the pool bigger.
        ///
        ///This request only changes the amount of bytes that are mmapped
        ///by the server and does not touch the file corresponding to the
        ///file descriptor passed at creation time. It is the client's
        ///responsibility to ensure that the file is at least as big as
        ///the new pool size.
        pub fn resize(sender_id: ObjectId, size: i32) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 2);
            wire_msg_builder.add_i32(size);
            wire_msg_builder.send()
        }
    }
}
///shared memory support
///
///A singleton global object that provides support for shared
///memory.
///
///Clients can create wl_shm_pool objects using the create_pool
///request.
///
///On binding the wl_shm object one or more format events
///are emitted to inform clients about the valid pixel formats
///that can be used for buffers.
pub mod wl_shm {
    use super::*;

    pub trait EvHandler {
        ///pixel format description
        ///
        ///Informs the client about a valid pixel format that
        ///can be used for buffers. Known formats include
        ///argb8888 and xrgb8888.
        fn format(&mut self, format: u32);
    }

    pub fn event<T: EvHandler>(state: &mut T, mut wire_msg: WireMsg, payload: WaylandPayload) {
        match wire_msg.op() {
            0 => state.format(wire_msg.next_u32(&payload)),
            e => log::error!("unrecognized event opcode: {e} for interface wl_shm"),
        }
    }

    ///Requests for this interface
    pub mod req {
        use super::*;
        ///create a shm pool
        ///
        ///Create a new wl_shm_pool object.
        ///
        ///The pool can be used to create shared memory based buffer
        ///objects.  The server will mmap size bytes of the passed file
        ///descriptor, to use as backing memory for the pool.
        pub fn create_pool(
            id: ObjectId,
            fd: &impl rustix::fd::AsRawFd,
            size: i32,
        ) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(globals::WL_SHM, 0);
            wire_msg_builder.add_new_specified_id(id);
            wire_msg_builder.add_fd(fd);
            wire_msg_builder.add_i32(size);
            wire_msg_builder.send()
        }
    }
    pub mod error {
        ///buffer format is not known
        pub const INVALID_FORMAT: u32 = 0u32;
        ///invalid size or stride during pool or buffer creation
        pub const INVALID_STRIDE: u32 = 1u32;
        ///mmapping the file descriptor failed
        pub const INVALID_FD: u32 = 2u32;
    }
    ///pixel formats
    ///
    ///This describes the memory layout of an individual pixel.
    ///
    ///All renderers should support argb8888 and xrgb8888 but any other
    ///formats are optional and may not be supported by the particular
    ///renderer in use.
    ///
    ///The drm format codes match the macros defined in drm_fourcc.h, except
    ///argb8888 and xrgb8888. The formats actually supported by the compositor
    ///will be reported by the format event.
    ///
    ///For all wl_shm formats and unless specified in another protocol
    ///extension, pre-multiplied alpha is used for pixel values.
    pub mod format {
        ///32-bit ARGB format, [31:0] A:R:G:B 8:8:8:8 little endian
        pub const ARGB8888: u32 = 0u32;
        ///32-bit RGB format, [31:0] x:R:G:B 8:8:8:8 little endian
        pub const XRGB8888: u32 = 1u32;
        ///8-bit color index format, [7:0] C
        pub const C8: u32 = 538982467u32;
        ///8-bit RGB format, [7:0] R:G:B 3:3:2
        pub const RGB332: u32 = 943867730u32;
        ///8-bit BGR format, [7:0] B:G:R 2:3:3
        pub const BGR233: u32 = 944916290u32;
        ///16-bit xRGB format, [15:0] x:R:G:B 4:4:4:4 little endian
        pub const XRGB4444: u32 = 842093144u32;
        ///16-bit xBGR format, [15:0] x:B:G:R 4:4:4:4 little endian
        pub const XBGR4444: u32 = 842089048u32;
        ///16-bit RGBx format, [15:0] R:G:B:x 4:4:4:4 little endian
        pub const RGBX4444: u32 = 842094674u32;
        ///16-bit BGRx format, [15:0] B:G:R:x 4:4:4:4 little endian
        pub const BGRX4444: u32 = 842094658u32;
        ///16-bit ARGB format, [15:0] A:R:G:B 4:4:4:4 little endian
        pub const ARGB4444: u32 = 842093121u32;
        ///16-bit ABGR format, [15:0] A:B:G:R 4:4:4:4 little endian
        pub const ABGR4444: u32 = 842089025u32;
        ///16-bit RBGA format, [15:0] R:G:B:A 4:4:4:4 little endian
        pub const RGBA4444: u32 = 842088786u32;
        ///16-bit BGRA format, [15:0] B:G:R:A 4:4:4:4 little endian
        pub const BGRA4444: u32 = 842088770u32;
        ///16-bit xRGB format, [15:0] x:R:G:B 1:5:5:5 little endian
        pub const XRGB1555: u32 = 892424792u32;
        ///16-bit xBGR 1555 format, [15:0] x:B:G:R 1:5:5:5 little endian
        pub const XBGR1555: u32 = 892420696u32;
        ///16-bit RGBx 5551 format, [15:0] R:G:B:x 5:5:5:1 little endian
        pub const RGBX5551: u32 = 892426322u32;
        ///16-bit BGRx 5551 format, [15:0] B:G:R:x 5:5:5:1 little endian
        pub const BGRX5551: u32 = 892426306u32;
        ///16-bit ARGB 1555 format, [15:0] A:R:G:B 1:5:5:5 little endian
        pub const ARGB1555: u32 = 892424769u32;
        ///16-bit ABGR 1555 format, [15:0] A:B:G:R 1:5:5:5 little endian
        pub const ABGR1555: u32 = 892420673u32;
        ///16-bit RGBA 5551 format, [15:0] R:G:B:A 5:5:5:1 little endian
        pub const RGBA5551: u32 = 892420434u32;
        ///16-bit BGRA 5551 format, [15:0] B:G:R:A 5:5:5:1 little endian
        pub const BGRA5551: u32 = 892420418u32;
        ///16-bit RGB 565 format, [15:0] R:G:B 5:6:5 little endian
        pub const RGB565: u32 = 909199186u32;
        ///16-bit BGR 565 format, [15:0] B:G:R 5:6:5 little endian
        pub const BGR565: u32 = 909199170u32;
        ///24-bit RGB format, [23:0] R:G:B little endian
        pub const RGB888: u32 = 875710290u32;
        ///24-bit BGR format, [23:0] B:G:R little endian
        pub const BGR888: u32 = 875710274u32;
        ///32-bit xBGR format, [31:0] x:B:G:R 8:8:8:8 little endian
        pub const XBGR8888: u32 = 875709016u32;
        ///32-bit RGBx format, [31:0] R:G:B:x 8:8:8:8 little endian
        pub const RGBX8888: u32 = 875714642u32;
        ///32-bit BGRx format, [31:0] B:G:R:x 8:8:8:8 little endian
        pub const BGRX8888: u32 = 875714626u32;
        ///32-bit ABGR format, [31:0] A:B:G:R 8:8:8:8 little endian
        pub const ABGR8888: u32 = 875708993u32;
        ///32-bit RGBA format, [31:0] R:G:B:A 8:8:8:8 little endian
        pub const RGBA8888: u32 = 875708754u32;
        ///32-bit BGRA format, [31:0] B:G:R:A 8:8:8:8 little endian
        pub const BGRA8888: u32 = 875708738u32;
        ///32-bit xRGB format, [31:0] x:R:G:B 2:10:10:10 little endian
        pub const XRGB2101010: u32 = 808669784u32;
        ///32-bit xBGR format, [31:0] x:B:G:R 2:10:10:10 little endian
        pub const XBGR2101010: u32 = 808665688u32;
        ///32-bit RGBx format, [31:0] R:G:B:x 10:10:10:2 little endian
        pub const RGBX1010102: u32 = 808671314u32;
        ///32-bit BGRx format, [31:0] B:G:R:x 10:10:10:2 little endian
        pub const BGRX1010102: u32 = 808671298u32;
        ///32-bit ARGB format, [31:0] A:R:G:B 2:10:10:10 little endian
        pub const ARGB2101010: u32 = 808669761u32;
        ///32-bit ABGR format, [31:0] A:B:G:R 2:10:10:10 little endian
        pub const ABGR2101010: u32 = 808665665u32;
        ///32-bit RGBA format, [31:0] R:G:B:A 10:10:10:2 little endian
        pub const RGBA1010102: u32 = 808665426u32;
        ///32-bit BGRA format, [31:0] B:G:R:A 10:10:10:2 little endian
        pub const BGRA1010102: u32 = 808665410u32;
        ///packed YCbCr format, [31:0] Cr0:Y1:Cb0:Y0 8:8:8:8 little endian
        pub const YUYV: u32 = 1448695129u32;
        ///packed YCbCr format, [31:0] Cb0:Y1:Cr0:Y0 8:8:8:8 little endian
        pub const YVYU: u32 = 1431918169u32;
        ///packed YCbCr format, [31:0] Y1:Cr0:Y0:Cb0 8:8:8:8 little endian
        pub const UYVY: u32 = 1498831189u32;
        ///packed YCbCr format, [31:0] Y1:Cb0:Y0:Cr0 8:8:8:8 little endian
        pub const VYUY: u32 = 1498765654u32;
        ///packed AYCbCr format, [31:0] A:Y:Cb:Cr 8:8:8:8 little endian
        pub const AYUV: u32 = 1448433985u32;
        ///2 plane YCbCr Cr:Cb format, 2x2 subsampled Cr:Cb plane
        pub const NV12: u32 = 842094158u32;
        ///2 plane YCbCr Cb:Cr format, 2x2 subsampled Cb:Cr plane
        pub const NV21: u32 = 825382478u32;
        ///2 plane YCbCr Cr:Cb format, 2x1 subsampled Cr:Cb plane
        pub const NV16: u32 = 909203022u32;
        ///2 plane YCbCr Cb:Cr format, 2x1 subsampled Cb:Cr plane
        pub const NV61: u32 = 825644622u32;
        ///3 plane YCbCr format, 4x4 subsampled Cb (1) and Cr (2) planes
        pub const YUV410: u32 = 961959257u32;
        ///3 plane YCbCr format, 4x4 subsampled Cr (1) and Cb (2) planes
        pub const YVU410: u32 = 961893977u32;
        ///3 plane YCbCr format, 4x1 subsampled Cb (1) and Cr (2) planes
        pub const YUV411: u32 = 825316697u32;
        ///3 plane YCbCr format, 4x1 subsampled Cr (1) and Cb (2) planes
        pub const YVU411: u32 = 825316953u32;
        ///3 plane YCbCr format, 2x2 subsampled Cb (1) and Cr (2) planes
        pub const YUV420: u32 = 842093913u32;
        ///3 plane YCbCr format, 2x2 subsampled Cr (1) and Cb (2) planes
        pub const YVU420: u32 = 842094169u32;
        ///3 plane YCbCr format, 2x1 subsampled Cb (1) and Cr (2) planes
        pub const YUV422: u32 = 909202777u32;
        ///3 plane YCbCr format, 2x1 subsampled Cr (1) and Cb (2) planes
        pub const YVU422: u32 = 909203033u32;
        ///3 plane YCbCr format, non-subsampled Cb (1) and Cr (2) planes
        pub const YUV444: u32 = 875713881u32;
        ///3 plane YCbCr format, non-subsampled Cr (1) and Cb (2) planes
        pub const YVU444: u32 = 875714137u32;
        ///[7:0] R
        pub const R8: u32 = 538982482u32;
        ///[15:0] R little endian
        pub const R16: u32 = 540422482u32;
        ///[15:0] R:G 8:8 little endian
        pub const RG88: u32 = 943212370u32;
        ///[15:0] G:R 8:8 little endian
        pub const GR88: u32 = 943215175u32;
        ///[31:0] R:G 16:16 little endian
        pub const RG1616: u32 = 842221394u32;
        ///[31:0] G:R 16:16 little endian
        pub const GR1616: u32 = 842224199u32;
        ///[63:0] x:R:G:B 16:16:16:16 little endian
        pub const XRGB16161616F: u32 = 1211388504u32;
        ///[63:0] x:B:G:R 16:16:16:16 little endian
        pub const XBGR16161616F: u32 = 1211384408u32;
        ///[63:0] A:R:G:B 16:16:16:16 little endian
        pub const ARGB16161616F: u32 = 1211388481u32;
        ///[63:0] A:B:G:R 16:16:16:16 little endian
        pub const ABGR16161616F: u32 = 1211384385u32;
        ///[31:0] X:Y:Cb:Cr 8:8:8:8 little endian
        pub const XYUV8888: u32 = 1448434008u32;
        ///[23:0] Cr:Cb:Y 8:8:8 little endian
        pub const VUY888: u32 = 875713878u32;
        ///Y followed by U then V, 10:10:10. Non-linear modifier only
        pub const VUY101010: u32 = 808670550u32;
        ///[63:0] Cr0:0:Y1:0:Cb0:0:Y0:0 10:6:10:6:10:6:10:6 little endian per 2 Y pixels
        pub const Y210: u32 = 808530521u32;
        ///[63:0] Cr0:0:Y1:0:Cb0:0:Y0:0 12:4:12:4:12:4:12:4 little endian per 2 Y pixels
        pub const Y212: u32 = 842084953u32;
        ///[63:0] Cr0:Y1:Cb0:Y0 16:16:16:16 little endian per 2 Y pixels
        pub const Y216: u32 = 909193817u32;
        ///[31:0] A:Cr:Y:Cb 2:10:10:10 little endian
        pub const Y410: u32 = 808531033u32;
        ///[63:0] A:0:Cr:0:Y:0:Cb:0 12:4:12:4:12:4:12:4 little endian
        pub const Y412: u32 = 842085465u32;
        ///[63:0] A:Cr:Y:Cb 16:16:16:16 little endian
        pub const Y416: u32 = 909194329u32;
        ///[31:0] X:Cr:Y:Cb 2:10:10:10 little endian
        pub const XVYU2101010: u32 = 808670808u32;
        ///[63:0] X:0:Cr:0:Y:0:Cb:0 12:4:12:4:12:4:12:4 little endian
        pub const XVYU12_16161616: u32 = 909334104u32;
        ///[63:0] X:Cr:Y:Cb 16:16:16:16 little endian
        pub const XVYU16161616: u32 = 942954072u32;
        ///[63:0]   A3:A2:Y3:0:Cr0:0:Y2:0:A1:A0:Y1:0:Cb0:0:Y0:0  1:1:8:2:8:2:8:2:1:1:8:2:8:2:8:2
        /// little endian
        pub const Y0L0: u32 = 810299481u32;
        ///[63:0]   X3:X2:Y3:0:Cr0:0:Y2:0:X1:X0:Y1:0:Cb0:0:Y0:0  1:1:8:2:8:2:8:2:1:1:8:2:8:2:8:2
        /// little endian
        pub const X0L0: u32 = 810299480u32;
        ///[63:0]   A3:A2:Y3:Cr0:Y2:A1:A0:Y1:Cb0:Y0  1:1:10:10:10:1:1:10:10:10 little endian
        pub const Y0L2: u32 = 843853913u32;
        ///[63:0]   X3:X2:Y3:Cr0:Y2:X1:X0:Y1:Cb0:Y0  1:1:10:10:10:1:1:10:10:10 little endian
        pub const X0L2: u32 = 843853912u32;
        pub const YUV420_8BIT: u32 = 942691673u32;
        pub const YUV420_10BIT: u32 = 808539481u32;
        pub const XRGB8888_A8: u32 = 943805016u32;
        pub const XBGR8888_A8: u32 = 943800920u32;
        pub const RGBX8888_A8: u32 = 943806546u32;
        pub const BGRX8888_A8: u32 = 943806530u32;
        pub const RGB888_A8: u32 = 943798354u32;
        pub const BGR888_A8: u32 = 943798338u32;
        pub const RGB565_A8: u32 = 943797586u32;
        pub const BGR565_A8: u32 = 943797570u32;
        ///non-subsampled Cr:Cb plane
        pub const NV24: u32 = 875714126u32;
        ///non-subsampled Cb:Cr plane
        pub const NV42: u32 = 842290766u32;
        ///2x1 subsampled Cr:Cb plane, 10 bit per channel
        pub const P210: u32 = 808530512u32;
        ///2x2 subsampled Cr:Cb plane 10 bits per channel
        pub const P010: u32 = 808530000u32;
        ///2x2 subsampled Cr:Cb plane 12 bits per channel
        pub const P012: u32 = 842084432u32;
        ///2x2 subsampled Cr:Cb plane 16 bits per channel
        pub const P016: u32 = 909193296u32;
        ///[63:0] A:x:B:x:G:x:R:x 10:6:10:6:10:6:10:6 little endian
        pub const AXBXGXRX106106106106: u32 = 808534593u32;
        ///2x2 subsampled Cr:Cb plane
        pub const NV15: u32 = 892425806u32;
        pub const Q410: u32 = 808531025u32;
        pub const Q401: u32 = 825242705u32;
        ///[63:0] x:R:G:B 16:16:16:16 little endian
        pub const XRGB16161616: u32 = 942953048u32;
        ///[63:0] x:B:G:R 16:16:16:16 little endian
        pub const XBGR16161616: u32 = 942948952u32;
        ///[63:0] A:R:G:B 16:16:16:16 little endian
        pub const ARGB16161616: u32 = 942953025u32;
        ///[63:0] A:B:G:R 16:16:16:16 little endian
        pub const ABGR16161616: u32 = 942948929u32;
    }
}
///content for a wl_surface
///
///A buffer provides the content for a wl_surface. Buffers are
///created through factory interfaces such as wl_shm, wp_linux_buffer_params
///(from the linux-dmabuf protocol extension) or similar. It has a width and
///a height and can be attached to a wl_surface, but the mechanism by which a
///client provides and updates the contents is defined by the buffer factory
///interface.
///
///If the buffer uses a format that has an alpha channel, the alpha channel
///is assumed to be premultiplied in the color channels unless otherwise
///specified.
///
///Note, because wl_buffer objects are created from multiple independent
///factory interfaces, the wl_buffer interface is frozen at version 1.
pub mod wl_buffer {
    use super::*;

    pub trait EvHandler {
        ///compositor releases buffer
        ///
        ///Sent when this wl_buffer is no longer used by the compositor.
        ///The client is now free to reuse or destroy this buffer and its
        ///backing storage.
        ///
        ///If a client receives a release event before the frame callback
        ///requested in the same wl_surface.commit that attaches this
        ///wl_buffer to a surface, then the client is immediately free to
        ///reuse the buffer and its backing storage, and does not need a
        ///second buffer for the next surface content update. Typically
        ///this is possible, when the compositor maintains a copy of the
        ///wl_surface contents, e.g. as a GL texture. This is an important
        ///optimization for GL(ES) compositors with wl_shm clients.
        fn release(&mut self, sender_id: ObjectId);
    }

    pub fn event<T: EvHandler>(state: &mut T, wire_msg: WireMsg, _payload: WaylandPayload) {
        match wire_msg.op() {
            0 => state.release(wire_msg.sender_id()),
            e => log::error!("unrecognized event opcode: {e} for interface wl_buffer"),
        }
    }
    ///Requests for this interface
    pub mod req {
        use super::*;
        ///destroy a buffer
        ///
        ///Destroy a buffer. If and how you need to release the backing
        ///storage is defined by the buffer factory interface.
        ///
        ///For possible side-effects to a surface, see wl_surface.attach.
        ///
        ///THIS IS A DESTRUCTOR
        pub fn destroy(sender_id: ObjectId) -> rustix::io::Result<()> {
            let wire_msg_builder = WireMsgBuilder::new(sender_id, 0);
            wire_msg_builder.send()
        }
    }
}
///an onscreen surface
///
///A surface is a rectangular area that may be displayed on zero
///or more outputs, and shown any number of times at the compositor's
///discretion. They can present wl_buffers, receive user input, and
///define a local coordinate system.
///
///The size of a surface (and relative positions on it) is described
///in surface-local coordinates, which may differ from the buffer
///coordinates of the pixel content, in case a buffer_transform
///or a buffer_scale is used.
///
///A surface without a "role" is fairly useless: a compositor does
///not know where, when or how to present it. The role is the
///purpose of a wl_surface. Examples of roles are a cursor for a
///pointer (as set by wl_pointer.set_cursor), a drag icon
///(wl_data_device.start_drag), a sub-surface
///(wl_subcompositor.get_subsurface), and a window as defined by a
///shell protocol (e.g. wl_shell.get_shell_surface).
///
///A surface can have only one role at a time. Initially a
///wl_surface does not have a role. Once a wl_surface is given a
///role, it is set permanently for the whole lifetime of the
///wl_surface object. Giving the current role again is allowed,
///unless explicitly forbidden by the relevant interface
///specification.
///
///Surface roles are given by requests in other interfaces such as
///wl_pointer.set_cursor. The request should explicitly mention
///that this request gives a role to a wl_surface. Often, this
///request also creates a new protocol object that represents the
///role and adds additional functionality to wl_surface. When a
///client wants to destroy a wl_surface, they must destroy this role
///object before the wl_surface, otherwise a defunct_role_object error is
///sent.
///
///Destroying the role object does not remove the role from the
///wl_surface, but it may stop the wl_surface from "playing the role".
///For instance, if a wl_subsurface object is destroyed, the wl_surface
///it was created for will be unmapped and forget its position and
///z-order. It is allowed to create a wl_subsurface for the same
///wl_surface again, but it is not allowed to use the wl_surface as
///a cursor (cursor is a different role than sub-surface, and role
///switching is not allowed).
pub mod wl_surface {
    use super::*;

    pub trait EvHandler {
        ///surface enters an output
        ///
        ///This is emitted whenever a surface's creation, movement, or resizing
        ///results in some part of it being within the scanout region of an
        ///output.
        ///
        ///Note that a surface may be overlapping with zero or more outputs.
        fn enter(&mut self, sender_id: ObjectId, output: ObjectId);
        ///surface leaves an output
        ///
        ///This is emitted whenever a surface's creation, movement, or resizing
        ///results in it no longer having any part of it within the scanout region
        ///of an output.
        ///
        ///Clients should not use the number of outputs the surface is on for frame
        ///throttling purposes. The surface might be hidden even if no leave event
        ///has been sent, and the compositor might expect new surface content
        ///updates even if no enter event has been sent. The frame event should be
        ///used instead.
        fn leave(&mut self, sender_id: ObjectId, output: ObjectId);
        ///preferred buffer scale for the surface
        ///
        ///This event indicates the preferred buffer scale for this surface. It is
        ///sent whenever the compositor's preference changes.
        ///
        ///It is intended that scaling aware clients use this event to scale their
        ///content and use wl_surface.set_buffer_scale to indicate the scale they
        ///have rendered with. This allows clients to supply a higher detail
        ///buffer.
        fn preferred_buffer_scale(&mut self, sender_id: ObjectId, factor: i32);
        ///preferred buffer transform for the surface
        ///
        ///This event indicates the preferred buffer transform for this surface.
        ///It is sent whenever the compositor's preference changes.
        ///
        ///It is intended that transform aware clients use this event to apply the
        ///transform to their content and use wl_surface.set_buffer_transform to
        ///indicate the transform they have rendered with.
        fn preferred_buffer_transform(&mut self, sender_id: ObjectId, transform: u32);
    }
    pub fn event<T: EvHandler>(state: &mut T, mut wire_msg: WireMsg, payload: WaylandPayload) {
        match wire_msg.op() {
            0 => {
                let output = wire_msg.next_object(&payload).unwrap();
                state.enter(wire_msg.sender_id(), output);
            }
            1 => {
                let output = wire_msg.next_object(&payload).unwrap();
                state.leave(wire_msg.sender_id(), output);
            }
            2 => {
                let factor = wire_msg.next_i32(&payload);
                state.preferred_buffer_scale(wire_msg.sender_id(), factor);
            }
            3 => {
                let transform = wire_msg.next_u32(&payload);
                state.preferred_buffer_transform(wire_msg.sender_id(), transform);
            }
            e => log::error!("unrecognized event opcode: {e} for interface wl_surface"),
        }
    }
    ///Requests for this interface
    pub mod req {
        use super::*;
        ///delete surface
        ///
        ///Deletes the surface and invalidates its object ID.
        ///
        ///THIS IS A DESTRUCTOR
        pub fn destroy(sender_id: ObjectId) -> rustix::io::Result<()> {
            let wire_msg_builder = WireMsgBuilder::new(sender_id, 0);
            wire_msg_builder.send()
        }
        ///set the surface contents
        ///
        ///Set a buffer as the content of this surface.
        ///
        ///The new size of the surface is calculated based on the buffer
        ///size transformed by the inverse buffer_transform and the
        ///inverse buffer_scale. This means that at commit time the supplied
        ///buffer size must be an integer multiple of the buffer_scale. If
        ///that's not the case, an invalid_size error is sent.
        ///
        ///The x and y arguments specify the location of the new pending
        ///buffer's upper left corner, relative to the current buffer's upper
        ///left corner, in surface-local coordinates. In other words, the
        ///x and y, combined with the new surface size define in which
        ///directions the surface's size changes. Setting anything other than 0
        ///as x and y arguments is discouraged, and should instead be replaced
        ///with using the separate wl_surface.offset request.
        ///
        ///When the bound wl_surface version is 5 or higher, passing any
        ///non-zero x or y is a protocol violation, and will result in an
        ///'invalid_offset' error being raised. The x and y arguments are ignored
        ///and do not change the pending state. To achieve equivalent semantics,
        ///use wl_surface.offset.
        ///
        ///Surface contents are double-buffered state, see wl_surface.commit.
        ///
        ///The initial surface contents are void; there is no content.
        ///wl_surface.attach assigns the given wl_buffer as the pending
        ///wl_buffer. wl_surface.commit makes the pending wl_buffer the new
        ///surface contents, and the size of the surface becomes the size
        ///calculated from the wl_buffer, as described above. After commit,
        ///there is no pending buffer until the next attach.
        ///
        ///Committing a pending wl_buffer allows the compositor to read the
        ///pixels in the wl_buffer. The compositor may access the pixels at
        ///any time after the wl_surface.commit request. When the compositor
        ///will not access the pixels anymore, it will send the
        ///wl_buffer.release event. Only after receiving wl_buffer.release,
        ///the client may reuse the wl_buffer. A wl_buffer that has been
        ///attached and then replaced by another attach instead of committed
        ///will not receive a release event, and is not used by the
        ///compositor.
        ///
        ///If a pending wl_buffer has been committed to more than one wl_surface,
        ///the delivery of wl_buffer.release events becomes undefined. A well
        ///behaved client should not rely on wl_buffer.release events in this
        ///case. Alternatively, a client could create multiple wl_buffer objects
        ///from the same backing storage or use wp_linux_buffer_release.
        ///
        ///Destroying the wl_buffer after wl_buffer.release does not change
        ///the surface contents. Destroying the wl_buffer before wl_buffer.release
        ///is allowed as long as the underlying buffer storage isn't re-used (this
        ///can happen e.g. on client process termination). However, if the client
        ///destroys the wl_buffer before receiving the wl_buffer.release event and
        ///mutates the underlying buffer storage, the surface contents become
        ///undefined immediately.
        ///
        ///If wl_surface.attach is sent with a NULL wl_buffer, the
        ///following wl_surface.commit will remove the surface content.
        pub fn attach(
            sender_id: ObjectId,
            buffer: Option<ObjectId>,
            x: i32,
            y: i32,
        ) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 1);
            wire_msg_builder.add_object(buffer);
            wire_msg_builder.add_i32(x);
            wire_msg_builder.add_i32(y);
            wire_msg_builder.send()
        }
        ///mark part of the surface damaged
        ///
        ///This request is used to describe the regions where the pending
        ///buffer is different from the current surface contents, and where
        ///the surface therefore needs to be repainted. The compositor
        ///ignores the parts of the damage that fall outside of the surface.
        ///
        ///Damage is double-buffered state, see wl_surface.commit.
        ///
        ///The damage rectangle is specified in surface-local coordinates,
        ///where x and y specify the upper left corner of the damage rectangle.
        ///
        ///The initial value for pending damage is empty: no damage.
        ///wl_surface.damage adds pending damage: the new pending damage
        ///is the union of old pending damage and the given rectangle.
        ///
        ///wl_surface.commit assigns pending damage as the current damage,
        ///and clears pending damage. The server will clear the current
        ///damage as it repaints the surface.
        ///
        ///Note! New clients should not use this request. Instead damage can be
        ///posted with wl_surface.damage_buffer which uses buffer coordinates
        ///instead of surface coordinates.
        pub fn damage(
            sender_id: ObjectId,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
        ) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 2);
            wire_msg_builder.add_i32(x);
            wire_msg_builder.add_i32(y);
            wire_msg_builder.add_i32(width);
            wire_msg_builder.add_i32(height);
            wire_msg_builder.send()
        }
        ///request a frame throttling hint
        ///
        ///Request a notification when it is a good time to start drawing a new
        ///frame, by creating a frame callback. This is useful for throttling
        ///redrawing operations, and driving animations.
        ///
        ///When a client is animating on a wl_surface, it can use the 'frame'
        ///request to get notified when it is a good time to draw and commit the
        ///next frame of animation. If the client commits an update earlier than
        ///that, it is likely that some updates will not make it to the display,
        ///and the client is wasting resources by drawing too often.
        ///
        ///The frame request will take effect on the next wl_surface.commit.
        ///The notification will only be posted for one frame unless
        ///requested again. For a wl_surface, the notifications are posted in
        ///the order the frame requests were committed.
        ///
        ///The server must send the notifications so that a client
        ///will not send excessive updates, while still allowing
        ///the highest possible update rate for clients that wait for the reply
        ///before drawing again. The server should give some time for the client
        ///to draw and commit after sending the frame callback events to let it
        ///hit the next output refresh.
        ///
        ///A server should avoid signaling the frame callbacks if the
        ///surface is not visible in any way, e.g. the surface is off-screen,
        ///or completely obscured by other opaque surfaces.
        ///
        ///The object returned by this request will be destroyed by the
        ///compositor after the callback is fired and as such the client must not
        ///attempt to use it after that point.
        ///
        ///The callback_data passed in the callback is the current time, in
        ///milliseconds, with an undefined base.
        pub fn frame(sender_id: ObjectId, callback: ObjectId) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 3);
            wire_msg_builder.add_new_specified_id(callback);
            wire_msg_builder.send()
        }
        ///set opaque region
        ///
        ///This request sets the region of the surface that contains
        ///opaque content.
        ///
        ///The opaque region is an optimization hint for the compositor
        ///that lets it optimize the redrawing of content behind opaque
        ///regions.  Setting an opaque region is not required for correct
        ///behaviour, but marking transparent content as opaque will result
        ///in repaint artifacts.
        ///
        ///The opaque region is specified in surface-local coordinates.
        ///
        ///The compositor ignores the parts of the opaque region that fall
        ///outside of the surface.
        ///
        ///Opaque region is double-buffered state, see wl_surface.commit.
        ///
        ///wl_surface.set_opaque_region changes the pending opaque region.
        ///wl_surface.commit copies the pending region to the current region.
        ///Otherwise, the pending and current regions are never changed.
        ///
        ///The initial value for an opaque region is empty. Setting the pending
        ///opaque region has copy semantics, and the wl_region object can be
        ///destroyed immediately. A NULL wl_region causes the pending opaque
        ///region to be set to empty.
        pub fn set_opaque_region(
            sender_id: ObjectId,
            region: Option<ObjectId>,
        ) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 4);
            wire_msg_builder.add_object(region);
            wire_msg_builder.send()
        }
        ///set input region
        ///
        ///This request sets the region of the surface that can receive
        ///pointer and touch events.
        ///
        ///Input events happening outside of this region will try the next
        ///surface in the server surface stack. The compositor ignores the
        ///parts of the input region that fall outside of the surface.
        ///
        ///The input region is specified in surface-local coordinates.
        ///
        ///Input region is double-buffered state, see wl_surface.commit.
        ///
        ///wl_surface.set_input_region changes the pending input region.
        ///wl_surface.commit copies the pending region to the current region.
        ///Otherwise the pending and current regions are never changed,
        ///except cursor and icon surfaces are special cases, see
        ///wl_pointer.set_cursor and wl_data_device.start_drag.
        ///
        ///The initial value for an input region is infinite. That means the
        ///whole surface will accept input. Setting the pending input region
        ///has copy semantics, and the wl_region object can be destroyed
        ///immediately. A NULL wl_region causes the input region to be set
        ///to infinite.
        pub fn set_input_region(
            sender_id: ObjectId,
            region: Option<ObjectId>,
        ) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 5);
            wire_msg_builder.add_object(region);
            wire_msg_builder.send()
        }
        ///commit pending surface state
        ///
        ///Surface state (input, opaque, and damage regions, attached buffers,
        ///etc.) is double-buffered. Protocol requests modify the pending state,
        ///as opposed to the current state in use by the compositor. A commit
        ///request atomically applies all pending state, replacing the current
        ///state. After commit, the new pending state is as documented for each
        ///related request.
        ///
        ///On commit, a pending wl_buffer is applied first, and all other state
        ///second. This means that all coordinates in double-buffered state are
        ///relative to the new wl_buffer coming into use, except for
        ///wl_surface.attach itself. If there is no pending wl_buffer, the
        ///coordinates are relative to the current surface contents.
        ///
        ///All requests that need a commit to become effective are documented
        ///to affect double-buffered state.
        ///
        ///Other interfaces may add further double-buffered surface state.
        pub fn commit(sender_id: ObjectId) -> rustix::io::Result<()> {
            let wire_msg_builder = WireMsgBuilder::new(sender_id, 6);
            wire_msg_builder.send()
        }
        ///sets the buffer transformation
        ///
        ///This request sets an optional transformation on how the compositor
        ///interprets the contents of the buffer attached to the surface. The
        ///accepted values for the transform parameter are the values for
        ///wl_output.transform.
        ///
        ///Buffer transform is double-buffered state, see wl_surface.commit.
        ///
        ///A newly created surface has its buffer transformation set to normal.
        ///
        ///wl_surface.set_buffer_transform changes the pending buffer
        ///transformation. wl_surface.commit copies the pending buffer
        ///transformation to the current one. Otherwise, the pending and current
        ///values are never changed.
        ///
        ///The purpose of this request is to allow clients to render content
        ///according to the output transform, thus permitting the compositor to
        ///use certain optimizations even if the display is rotated. Using
        ///hardware overlays and scanning out a client buffer for fullscreen
        ///surfaces are examples of such optimizations. Those optimizations are
        ///highly dependent on the compositor implementation, so the use of this
        ///request should be considered on a case-by-case basis.
        ///
        ///Note that if the transform value includes 90 or 270 degree rotation,
        ///the width of the buffer will become the surface height and the height
        ///of the buffer will become the surface width.
        ///
        ///If transform is not one of the values from the
        ///wl_output.transform enum the invalid_transform protocol error
        ///is raised.
        pub fn set_buffer_transform(sender_id: ObjectId, transform: i32) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 7);
            wire_msg_builder.add_i32(transform);
            wire_msg_builder.send()
        }
        ///sets the buffer scaling factor
        ///
        ///This request sets an optional scaling factor on how the compositor
        ///interprets the contents of the buffer attached to the window.
        ///
        ///Buffer scale is double-buffered state, see wl_surface.commit.
        ///
        ///A newly created surface has its buffer scale set to 1.
        ///
        ///wl_surface.set_buffer_scale changes the pending buffer scale.
        ///wl_surface.commit copies the pending buffer scale to the current one.
        ///Otherwise, the pending and current values are never changed.
        ///
        ///The purpose of this request is to allow clients to supply higher
        ///resolution buffer data for use on high resolution outputs. It is
        ///intended that you pick the same buffer scale as the scale of the
        ///output that the surface is displayed on. This means the compositor
        ///can avoid scaling when rendering the surface on that output.
        ///
        ///Note that if the scale is larger than 1, then you have to attach
        ///a buffer that is larger (by a factor of scale in each dimension)
        ///than the desired surface size.
        ///
        ///If scale is not positive the invalid_scale protocol error is
        ///raised.
        pub fn set_buffer_scale(sender_id: ObjectId, scale: i32) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 8);
            wire_msg_builder.add_i32(scale);
            wire_msg_builder.send()
        }
        ///mark part of the surface damaged using buffer coordinates
        ///
        ///This request is used to describe the regions where the pending
        ///buffer is different from the current surface contents, and where
        ///the surface therefore needs to be repainted. The compositor
        ///ignores the parts of the damage that fall outside of the surface.
        ///
        ///Damage is double-buffered state, see wl_surface.commit.
        ///
        ///The damage rectangle is specified in buffer coordinates,
        ///where x and y specify the upper left corner of the damage rectangle.
        ///
        ///The initial value for pending damage is empty: no damage.
        ///wl_surface.damage_buffer adds pending damage: the new pending
        ///damage is the union of old pending damage and the given rectangle.
        ///
        ///wl_surface.commit assigns pending damage as the current damage,
        ///and clears pending damage. The server will clear the current
        ///damage as it repaints the surface.
        ///
        ///This request differs from wl_surface.damage in only one way - it
        ///takes damage in buffer coordinates instead of surface-local
        ///coordinates. While this generally is more intuitive than surface
        ///coordinates, it is especially desirable when using wp_viewport
        ///or when a drawing library (like EGL) is unaware of buffer scale
        ///and buffer transform.
        ///
        ///Note: Because buffer transformation changes and damage requests may
        ///be interleaved in the protocol stream, it is impossible to determine
        ///the actual mapping between surface and buffer damage until
        ///wl_surface.commit time. Therefore, compositors wishing to take both
        ///kinds of damage into account will have to accumulate damage from the
        ///two requests separately and only transform from one to the other
        ///after receiving the wl_surface.commit.
        pub fn damage_buffer(
            sender_id: ObjectId,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
        ) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 9);
            wire_msg_builder.add_i32(x);
            wire_msg_builder.add_i32(y);
            wire_msg_builder.add_i32(width);
            wire_msg_builder.add_i32(height);
            wire_msg_builder.send()
        }
        ///set the surface contents offset
        ///
        ///The x and y arguments specify the location of the new pending
        ///buffer's upper left corner, relative to the current buffer's upper
        ///left corner, in surface-local coordinates. In other words, the
        ///x and y, combined with the new surface size define in which
        ///directions the surface's size changes.
        ///
        ///Surface location offset is double-buffered state, see
        ///wl_surface.commit.
        ///
        ///This request is semantically equivalent to and the replaces the x and y
        ///arguments in the wl_surface.attach request in wl_surface versions prior
        ///to 5. See wl_surface.attach for details.
        pub fn offset(sender_id: ObjectId, x: i32, y: i32) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 10);
            wire_msg_builder.add_i32(x);
            wire_msg_builder.add_i32(y);
            wire_msg_builder.send()
        }
    }
    pub mod error {
        ///buffer scale value is invalid
        pub const INVALID_SCALE: u32 = 0u32;
        ///buffer transform value is invalid
        pub const INVALID_TRANSFORM: u32 = 1u32;
        ///buffer size is invalid
        pub const INVALID_SIZE: u32 = 2u32;
        ///buffer offset is invalid
        pub const INVALID_OFFSET: u32 = 3u32;
        ///surface was destroyed before its role object
        pub const DEFUNCT_ROLE_OBJECT: u32 = 4u32;
    }
}
///compositor output region
///
///An output describes part of the compositor geometry.  The
///compositor works in the 'compositor coordinate system' and an
///output corresponds to a rectangular area in that space that is
///actually visible.  This typically corresponds to a monitor that
///displays part of the compositor space.  This object is published
///as global during start up, or when a monitor is hotplugged.
pub mod wl_output {
    use super::*;

    pub trait EvHandler {
        ///properties of the output
        ///
        ///The geometry event describes geometric properties of the output.
        ///The event is sent when binding to the output object and whenever
        ///any of the properties change.
        ///
        ///The physical size can be set to zero if it doesn't make sense for this
        ///output (e.g. for projectors or virtual outputs).
        ///
        ///The geometry event will be followed by a done event (starting from
        ///version 2).
        ///
        ///Note: wl_output only advertises partial information about the output
        ///position and identification. Some compositors, for instance those not
        ///implementing a desktop-style output layout or those exposing virtual
        ///outputs, might fake this information. Instead of using x and y, clients
        ///should use xdg_output.logical_position. Instead of using make and model,
        ///clients should use name and description.
        #[allow(clippy::too_many_arguments)]
        fn geometry(
            &mut self,
            sender_id: ObjectId,
            x: i32,
            y: i32,
            physical_width: i32,
            physical_height: i32,
            subpixel: i32,
            make: &str,
            model: &str,
            transform: i32,
        );
        ///advertise available modes for the output
        ///
        ///The mode event describes an available mode for the output.
        ///
        ///The event is sent when binding to the output object and there
        ///will always be one mode, the current mode.  The event is sent
        ///again if an output changes mode, for the mode that is now
        ///current.  In other words, the current mode is always the last
        ///mode that was received with the current flag set.
        ///
        ///Non-current modes are deprecated. A compositor can decide to only
        ///advertise the current mode and never send other modes. Clients
        ///should not rely on non-current modes.
        ///
        ///The size of a mode is given in physical hardware units of
        ///the output device. This is not necessarily the same as
        ///the output size in the global compositor space. For instance,
        ///the output may be scaled, as described in wl_output.scale,
        ///or transformed, as described in wl_output.transform. Clients
        ///willing to retrieve the output size in the global compositor
        ///space should use xdg_output.logical_size instead.
        ///
        ///The vertical refresh rate can be set to zero if it doesn't make
        ///sense for this output (e.g. for virtual outputs).
        ///
        ///The mode event will be followed by a done event (starting from
        ///version 2).
        ///
        ///Clients should not use the refresh rate to schedule frames. Instead,
        ///they should use the wl_surface.frame event or the presentation-time
        ///protocol.
        ///
        ///Note: this information is not always meaningful for all outputs. Some
        ///compositors, such as those exposing virtual outputs, might fake the
        ///refresh rate or the size.
        fn mode(&mut self, sender_id: ObjectId, flags: u32, width: i32, height: i32, refresh: i32);
        ///sent all information about output
        ///
        ///This event is sent after all other properties have been
        ///sent after binding to the output object and after any
        ///other property changes done after that. This allows
        ///changes to the output properties to be seen as
        ///atomic, even if they happen via multiple events.
        fn done(&mut self, sender_id: ObjectId);
        ///output scaling properties
        ///
        ///This event contains scaling geometry information
        ///that is not in the geometry event. It may be sent after
        ///binding the output object or if the output scale changes
        ///later. If it is not sent, the client should assume a
        ///scale of 1.
        ///
        ///A scale larger than 1 means that the compositor will
        ///automatically scale surface buffers by this amount
        ///when rendering. This is used for very high resolution
        ///displays where applications rendering at the native
        ///resolution would be too small to be legible.
        ///
        ///It is intended that scaling aware clients track the
        ///current output of a surface, and if it is on a scaled
        ///output it should use wl_surface.set_buffer_scale with
        ///the scale of the output. That way the compositor can
        ///avoid scaling the surface, and the client can supply
        ///a higher detail image.
        ///
        ///The scale event will be followed by a done event.
        fn scale(&mut self, sender_id: ObjectId, factor: i32);
        ///name of this output
        ///
        ///Many compositors will assign user-friendly names to their outputs, show
        ///them to the user, allow the user to refer to an output, etc. The client
        ///may wish to know this name as well to offer the user similar behaviors.
        ///
        ///The name is a UTF-8 string with no convention defined for its contents.
        ///Each name is unique among all wl_output globals. The name is only
        ///guaranteed to be unique for the compositor instance.
        ///
        ///The same output name is used for all clients for a given wl_output
        ///global. Thus, the name can be shared across processes to refer to a
        ///specific wl_output global.
        ///
        ///The name is not guaranteed to be persistent across sessions, thus cannot
        ///be used to reliably identify an output in e.g. configuration files.
        ///
        ///Examples of names include 'HDMI-A-1', 'WL-1', 'X11-1', etc. However, do
        ///not assume that the name is a reflection of an underlying DRM connector,
        ///X11 connection, etc.
        ///
        ///The name event is sent after binding the output object. This event is
        ///only sent once per output object, and the name does not change over the
        ///lifetime of the wl_output global.
        ///
        ///Compositors may re-use the same output name if the wl_output global is
        ///destroyed and re-created later. Compositors should avoid re-using the
        ///same name if possible.
        ///
        ///The name event will be followed by a done event.
        fn name(&mut self, sender_id: ObjectId, name: &str);
        ///human-readable description of this output
        ///
        ///Many compositors can produce human-readable descriptions of their
        ///outputs. The client may wish to know this description as well, e.g. for
        ///output selection purposes.
        ///
        ///The description is a UTF-8 string with no convention defined for its
        ///contents. The description is not guaranteed to be unique among all
        ///wl_output globals. Examples might include 'Foocorp 11" Display' or
        ///'Virtual X11 output via :1'.
        ///
        ///The description event is sent after binding the output object and
        ///whenever the description changes. The description is optional, and may
        ///not be sent at all.
        ///
        ///The description event will be followed by a done event.
        fn description(&mut self, sender_id: ObjectId, description: &str);
    }

    pub fn event<T: EvHandler>(state: &mut T, mut wire_msg: WireMsg, payload: WaylandPayload) {
        match wire_msg.op() {
            0 => {
                let x = wire_msg.next_i32(&payload);
                let y = wire_msg.next_i32(&payload);
                let physical_width = wire_msg.next_i32(&payload);
                let physical_height = wire_msg.next_i32(&payload);
                let subpixel = wire_msg.next_i32(&payload);
                let make = wire_msg.next_string(&payload);
                let model = wire_msg.next_string(&payload);
                let transform = wire_msg.next_i32(&payload);
                state.geometry(
                    wire_msg.sender_id(),
                    x,
                    y,
                    physical_width,
                    physical_height,
                    subpixel,
                    make,
                    model,
                    transform,
                );
            }
            1 => {
                let flags = wire_msg.next_u32(&payload);
                let width = wire_msg.next_i32(&payload);
                let height = wire_msg.next_i32(&payload);
                let refresh = wire_msg.next_i32(&payload);
                state.mode(wire_msg.sender_id(), flags, width, height, refresh);
            }
            2 => {
                state.done(wire_msg.sender_id());
            }
            3 => {
                let factor = wire_msg.next_i32(&payload);
                state.scale(wire_msg.sender_id(), factor);
            }
            4 => {
                let name = wire_msg.next_string(&payload);
                state.name(wire_msg.sender_id(), name);
            }
            5 => {
                let description = wire_msg.next_string(&payload);
                state.description(wire_msg.sender_id(), description);
            }
            e => log::error!("unrecognized event opcode: {e} for interface wl_output"),
        }
    }

    ///Requests for this interface
    pub mod req {
        use super::*;
        ///release the output object
        ///
        ///Using this request a client can tell the server that it is not going to
        ///use the output object anymore.
        ///
        ///THIS IS A DESTRUCTOR
        pub fn release(sender_id: ObjectId) -> rustix::io::Result<()> {
            let wire_msg_builder = WireMsgBuilder::new(sender_id, 0u16);
            wire_msg_builder.send()
        }
    }
    ///subpixel geometry information
    ///
    ///This enumeration describes how the physical
    ///pixels on an output are laid out.
    pub mod subpixel {
        ///unknown geometry
        pub const UNKNOWN: u32 = 0u32;
        ///no geometry
        pub const NONE: u32 = 1u32;
        ///horizontal RGB
        pub const HORIZONTAL_RGB: u32 = 2u32;
        ///horizontal BGR
        pub const HORIZONTAL_BGR: u32 = 3u32;
        ///vertical RGB
        pub const VERTICAL_RGB: u32 = 4u32;
        ///vertical BGR
        pub const VERTICAL_BGR: u32 = 5u32;
    }
    ///transform from framebuffer to output
    ///
    ///This describes the transform that a compositor will apply to a
    ///surface to compensate for the rotation or mirroring of an
    ///output device.
    ///
    ///The flipped values correspond to an initial flip around a
    ///vertical axis followed by rotation.
    ///
    ///The purpose is mainly to allow clients to render accordingly and
    ///tell the compositor, so that for fullscreen surfaces, the
    ///compositor will still be able to scan out directly from client
    ///surfaces.
    pub mod transform {
        ///no transform
        pub const NORMAL: u32 = 0u32;
        ///90 degrees counter-clockwise
        pub const _90: u32 = 1u32;
        ///180 degrees counter-clockwise
        pub const _180: u32 = 2u32;
        ///270 degrees counter-clockwise
        pub const _270: u32 = 3u32;
        ///180 degree flip around a vertical axis
        pub const FLIPPED: u32 = 4u32;
        ///flip and rotate 90 degrees counter-clockwise
        pub const FLIPPED_90: u32 = 5u32;
        ///flip and rotate 180 degrees counter-clockwise
        pub const FLIPPED_180: u32 = 6u32;
        ///flip and rotate 270 degrees counter-clockwise
        pub const FLIPPED_270: u32 = 7u32;
    }
    ///BITFIELD
    ///mode information
    ///
    ///These flags describe properties of an output mode.
    ///They are used in the flags bitfield of the mode event.
    pub mod mode {
        ///indicates this is the current mode
        pub const CURRENT: u32 = 1u32;
        ///indicates this is the preferred mode
        pub const PREFERRED: u32 = 2u32;
    }
}
///region interface
///
///A region object describes an area.
///
///Region objects are used to describe the opaque and input
///regions of a surface.
pub mod wl_region {
    use super::*;

    ///Events for this interface
    pub mod ev {}
    ///Requests for this interface
    pub mod req {
        use super::*;
        ///destroy region
        ///
        ///Destroy the region.  This will invalidate the object ID.
        ///
        ///THIS IS A DESTRUCTOR
        pub fn destroy(sender_id: ObjectId) -> rustix::io::Result<()> {
            let wire_msg_builder = WireMsgBuilder::new(sender_id, 0u16);
            wire_msg_builder.send()
        }
        ///add rectangle to region
        ///
        ///Add the specified rectangle to the region.
        pub fn add(
            sender_id: ObjectId,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
        ) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 1u16);
            wire_msg_builder.add_i32(x);
            wire_msg_builder.add_i32(y);
            wire_msg_builder.add_i32(width);
            wire_msg_builder.add_i32(height);
            wire_msg_builder.send()
        }
        ///subtract rectangle from region
        ///
        ///Subtract the specified rectangle from the region.
        pub fn subtract(
            sender_id: ObjectId,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
        ) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 2u16);
            wire_msg_builder.add_i32(x);
            wire_msg_builder.add_i32(y);
            wire_msg_builder.add_i32(width);
            wire_msg_builder.add_i32(height);
            wire_msg_builder.send()
        }
    }
}
///surface cropping and scaling
///
///The global interface exposing surface cropping and scaling
///capabilities is used to instantiate an interface extension for a
///wl_surface object. This extended interface will then allow
///cropping and scaling the surface contents, effectively
///disconnecting the direct relationship between the buffer and the
///surface size.
pub mod wp_viewporter {
    use super::*;

    ///Events for this interface
    pub mod ev {}
    ///Requests for this interface
    pub mod req {
        use super::*;
        ///unbind from the cropping and scaling interface
        ///
        ///Informs the server that the client will not be using this
        ///protocol object anymore. This does not affect any other objects,
        ///wp_viewport objects included.
        ///
        ///THIS IS A DESTRUCTOR
        pub fn destroy() -> rustix::io::Result<()> {
            let wire_msg_builder = WireMsgBuilder::new(globals::WP_VIEWPORTER, 0);
            wire_msg_builder.send()
        }
        ///extend surface interface for crop and scale
        ///
        ///Instantiate an interface extension for the given wl_surface to
        ///crop and scale its content. If the given wl_surface already has
        ///a wp_viewport object associated, the viewport_exists
        ///protocol error is raised.
        pub fn get_viewport(id: ObjectId, surface: ObjectId) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(globals::WP_VIEWPORTER, 1);
            wire_msg_builder.add_new_specified_id(id);
            wire_msg_builder.add_object(Some(surface));
            wire_msg_builder.send()
        }
    }
    pub mod error {
        ///the surface already has a viewport object associated
        pub const VIEWPORT_EXISTS: u32 = 0u32;
    }
}
///crop and scale interface to a wl_surface
///
///An additional interface to a wl_surface object, which allows the
///client to specify the cropping and scaling of the surface
///contents.
///
///This interface works with two concepts: the source rectangle (src_x,
///src_y, src_width, src_height), and the destination size (dst_width,
///dst_height). The contents of the source rectangle are scaled to the
///destination size, and content outside the source rectangle is ignored.
///This state is double-buffered, and is applied on the next
///wl_surface.commit.
///
///The two parts of crop and scale state are independent: the source
///rectangle, and the destination size. Initially both are unset, that
///is, no scaling is applied. The whole of the current wl_buffer is
///used as the source, and the surface size is as defined in
///wl_surface.attach.
///
///If the destination size is set, it causes the surface size to become
///dst_width, dst_height. The source (rectangle) is scaled to exactly
///this size. This overrides whatever the attached wl_buffer size is,
///unless the wl_buffer is NULL. If the wl_buffer is NULL, the surface
///has no content and therefore no size. Otherwise, the size is always
///at least 1x1 in surface local coordinates.
///
///If the source rectangle is set, it defines what area of the wl_buffer is
///taken as the source. If the source rectangle is set and the destination
///size is not set, then src_width and src_height must be integers, and the
///surface size becomes the source rectangle size. This results in cropping
///without scaling. If src_width or src_height are not integers and
///destination size is not set, the bad_size protocol error is raised when
///the surface state is applied.
///
///The coordinate transformations from buffer pixel coordinates up to
///the surface-local coordinates happen in the following order:
/// 1. buffer_transform (wl_surface.set_buffer_transform)
/// 2. buffer_scale (wl_surface.set_buffer_scale)
/// 3. crop and scale (wp_viewport.set*)
///This means, that the source rectangle coordinates of crop and scale
///are given in the coordinates after the buffer transform and scale,
///i.e. in the coordinates that would be the surface-local coordinates
///if the crop and scale was not applied.
///
///If src_x or src_y are negative, the bad_value protocol error is raised.
///Otherwise, if the source rectangle is partially or completely outside of
///the non-NULL wl_buffer, then the out_of_buffer protocol error is raised
///when the surface state is applied. A NULL wl_buffer does not raise the
///out_of_buffer error.
///
///If the wl_surface associated with the wp_viewport is destroyed,
///all wp_viewport requests except 'destroy' raise the protocol error
///no_surface.
///
///If the wp_viewport object is destroyed, the crop and scale
///state is removed from the wl_surface. The change will be applied
///on the next wl_surface.commit.
pub mod wp_viewport {
    use super::*;

    ///Events for this interface
    pub mod ev {}
    ///Requests for this interface
    pub mod req {
        use super::*;
        ///remove scaling and cropping from the surface
        ///
        ///The associated wl_surface's crop and scale state is removed.
        ///The change is applied on the next wl_surface.commit.
        ///
        ///THIS IS A DESTRUCTOR
        pub fn destroy(sender_id: ObjectId) -> rustix::io::Result<()> {
            let wire_msg_builder = WireMsgBuilder::new(sender_id, 0);
            wire_msg_builder.send()
        }
        ///set the source rectangle for cropping
        ///
        ///Set the source rectangle of the associated wl_surface. See
        ///wp_viewport for the description, and relation to the wl_buffer
        ///size.
        ///
        ///If all of x, y, width and height are -1.0, the source rectangle is
        ///unset instead. Any other set of values where width or height are zero
        ///or negative, or x or y are negative, raise the bad_value protocol
        ///error.
        ///
        ///The crop and scale state is double-buffered state, and will be
        ///applied on the next wl_surface.commit.
        pub fn set_source(
            sender_id: ObjectId,
            x: WlFixed,
            y: WlFixed,
            width: WlFixed,
            height: WlFixed,
        ) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 1);
            wire_msg_builder.add_fixed(x);
            wire_msg_builder.add_fixed(y);
            wire_msg_builder.add_fixed(width);
            wire_msg_builder.add_fixed(height);
            wire_msg_builder.send()
        }
        ///set the surface size for scaling
        ///
        ///Set the destination size of the associated wl_surface. See
        ///wp_viewport for the description, and relation to the wl_buffer
        ///size.
        ///
        ///If width is -1 and height is -1, the destination size is unset
        ///instead. Any other pair of values for width and height that
        ///contains zero or negative values raises the bad_value protocol
        ///error.
        ///
        ///The crop and scale state is double-buffered state, and will be
        ///applied on the next wl_surface.commit.
        pub fn set_destination(
            sender_id: ObjectId,

            width: i32,
            height: i32,
        ) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 2);
            wire_msg_builder.add_i32(width);
            wire_msg_builder.add_i32(height);
            wire_msg_builder.send()
        }
    }
    pub mod error {
        ///negative or zero values in width or height
        pub const BAD_VALUE: u32 = 0u32;
        ///destination size is not integer
        pub const BAD_SIZE: u32 = 1u32;
        ///source rectangle extends outside of the content area
        pub const OUT_OF_BUFFER: u32 = 2u32;
        ///the wl_surface was destroyed
        pub const NO_SURFACE: u32 = 3u32;
    }
}
///Protocol for requesting fractional surface scales
///
///This protocol allows a compositor to suggest for surfaces to render at
///fractional scales.
///
///A client can submit scaled content by utilizing wp_viewport. This is done by
///creating a wp_viewport object for the surface and setting the destination
///rectangle to the surface size before the scale factor is applied.
///
///The buffer size is calculated by multiplying the surface size by the
///intended scale.
///
///The wl_surface buffer scale should remain set to 1.
///
///If a surface has a surface-local size of 100 px by 50 px and wishes to
///submit buffers with a scale of 1.5, then a buffer of 150px by 75 px should
///be used and the wp_viewport destination rectangle should be 100 px by 50 px.
///
///For toplevel surfaces, the size is rounded halfway away from zero. The
///rounding algorithm for subsurface position and size is not defined.
///fractional surface scale information
///
///A global interface for requesting surfaces to use fractional scales.
pub mod wp_fractional_scale_manager_v1 {
    use super::*;

    ///Events for this interface
    pub mod ev {}
    ///Requests for this interface
    pub mod req {
        use super::*;
        ///unbind the fractional surface scale interface
        ///
        ///Informs the server that the client will not be using this protocol
        ///object anymore. This does not affect any other objects,
        ///wp_fractional_scale_v1 objects included.
        ///
        ///THIS IS A DESTRUCTOR
        pub fn destroy(sender_id: ObjectId) -> rustix::io::Result<()> {
            let wire_msg_builder = WireMsgBuilder::new(sender_id, 0);
            wire_msg_builder.send()
        }
        ///extend surface interface for scale information
        ///
        ///Create an add-on object for the the wl_surface to let the compositor
        ///request fractional scales. If the given wl_surface already has a
        ///wp_fractional_scale_v1 object associated, the fractional_scale_exists
        ///protocol error is raised.
        pub fn get_fractional_scale(
            sender_id: ObjectId,
            id: ObjectId,
            surface: ObjectId,
        ) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 1);
            wire_msg_builder.add_new_specified_id(id);
            wire_msg_builder.add_object(Some(surface));
            wire_msg_builder.send()
        }
    }
    pub mod error {
        ///the surface already has a fractional_scale object associated
        pub const FRACTIONAL_SCALE_EXISTS: u32 = 0u32;
    }
}
///fractional scale interface to a wl_surface
///
///An additional interface to a wl_surface object which allows the compositor
///to inform the client of the preferred scale.
pub mod wp_fractional_scale_v1 {
    use super::*;

    pub trait EvHandler {
        ///notify of new preferred scale
        ///
        ///Notification of a new preferred scale for this surface that the
        ///compositor suggests that the client should use.
        ///
        ///The sent scale is the numerator of a fraction with a denominator of 120.
        fn preferred_scale(&mut self, sender_id: ObjectId, scale: u32);
    }

    pub fn event<T: EvHandler>(state: &mut T, mut wire_msg: WireMsg, payload: WaylandPayload) {
        match wire_msg.op() {
            0 => {
                let scale = wire_msg.next_u32(&payload);
                state.preferred_scale(wire_msg.sender_id(), scale);
            }
            e => log::error!("unrecognized event opcode: {e} for interface wp_fractional_scale_v1"),
        }
    }

    ///Requests for this interface
    pub mod req {
        use super::*;
        ///remove surface scale information for surface
        ///
        ///Destroy the fractional scale object. When this object is destroyed,
        ///preferred_scale events will no longer be sent.
        ///
        ///THIS IS A DESTRUCTOR
        pub fn destroy(sender_id: ObjectId) -> rustix::io::Result<()> {
            let wire_msg_builder = WireMsgBuilder::new(sender_id, 0);
            wire_msg_builder.send()
        }
    }
}
///create surfaces that are layers of the desktop
///
///Clients can use this interface to assign the surface_layer role to
///wl_surfaces. Such surfaces are assigned to a "layer" of the output and
///rendered with a defined z-depth respective to each other. They may also be
///anchored to the edges and corners of a screen and specify input handling
///semantics. This interface should be suitable for the implementation of
///many desktop shell components, and a broad number of other applications
///that interact with the desktop.
pub mod zwlr_layer_shell_v1 {
    use super::*;

    ///Events for this interface
    pub mod ev {}
    ///Requests for this interface
    pub mod req {
        use super::*;
        ///create a layer_surface from a surface
        ///
        ///Create a layer surface for an existing surface. This assigns the role of
        ///layer_surface, or raises a protocol error if another role is already
        ///assigned.
        ///
        ///Creating a layer surface from a wl_surface which has a buffer attached
        ///or committed is a client error, and any attempts by a client to attach
        ///or manipulate a buffer prior to the first layer_surface.configure call
        ///must also be treated as errors.
        ///
        ///After creating a layer_surface object and setting it up, the client
        ///must perform an initial commit without any buffer attached.
        ///The compositor will reply with a layer_surface.configure event.
        ///The client must acknowledge it and is then allowed to attach a buffer
        ///to map the surface.
        ///
        ///You may pass NULL for output to allow the compositor to decide which
        ///output to use. Generally this will be the one that the user most
        ///recently interacted with.
        ///
        ///Clients can specify a namespace that defines the purpose of the layer
        ///surface.
        pub fn get_layer_surface(
            id: ObjectId,
            surface: ObjectId,
            output: Option<ObjectId>,
            layer: u32,
            namespace: &str,
        ) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(globals::ZWLR_LAYER_SHELL_V1, 0);
            wire_msg_builder.add_new_specified_id(id);
            wire_msg_builder.add_object(Some(surface));
            wire_msg_builder.add_object(output);
            wire_msg_builder.add_u32(layer);
            wire_msg_builder.add_string(namespace);
            wire_msg_builder.send()
        }
        ///destroy the layer_shell object
        ///
        ///This request indicates that the client will not use the layer_shell
        ///object any more. Objects that have been created through this instance
        ///are not affected.
        ///
        ///THIS IS A DESTRUCTOR
        pub fn destroy() -> rustix::io::Result<()> {
            let wire_msg_builder = WireMsgBuilder::new(globals::ZWLR_LAYER_SHELL_V1, 1);
            wire_msg_builder.send()
        }
    }
    pub mod error {
        ///wl_surface has another role
        pub const ROLE: u32 = 0u32;
        ///layer value is invalid
        pub const INVALID_LAYER: u32 = 1u32;
        ///wl_surface has a buffer attached or committed
        pub const ALREADY_CONSTRUCTED: u32 = 2u32;
    }
    ///available layers for surfaces
    ///
    ///These values indicate which layers a surface can be rendered in. They
    ///are ordered by z depth, bottom-most first. Traditional shell surfaces
    ///will typically be rendered between the bottom and top layers.
    ///Fullscreen shell surfaces are typically rendered at the top layer.
    ///Multiple surfaces can share a single layer, and ordering within a
    ///single layer is undefined.
    pub mod layer {
        pub const BACKGROUND: u32 = 0u32;
        pub const BOTTOM: u32 = 1u32;
        pub const TOP: u32 = 2u32;
        pub const OVERLAY: u32 = 3u32;
    }
}
///layer metadata interface
///
///An interface that may be implemented by a wl_surface, for surfaces that
///are designed to be rendered as a layer of a stacked desktop-like
///environment.
///
///Layer surface state (layer, size, anchor, exclusive zone,
///margin, interactivity) is double-buffered, and will be applied at the
///time wl_surface.commit of the corresponding wl_surface is called.
///
///Attaching a null buffer to a layer surface unmaps it.
///
///Unmapping a layer_surface means that the surface cannot be shown by the
///compositor until it is explicitly mapped again. The layer_surface
///returns to the state it had right after layer_shell.get_layer_surface.
///The client can re-map the surface by performing a commit without any
///buffer attached, waiting for a configure event and handling it as usual.
pub mod zwlr_layer_surface_v1 {
    use super::*;

    pub trait EvHandler {
        ///suggest a surface change
        ///
        ///The configure event asks the client to resize its surface.
        ///
        ///Clients should arrange their surface for the new states, and then send
        ///an ack_configure request with the serial sent in this configure event at
        ///some point before committing the new surface.
        ///
        ///The client is free to dismiss all but the last configure event it
        ///received.
        ///
        ///The width and height arguments specify the size of the window in
        ///surface-local coordinates.
        ///
        ///The size is a hint, in the sense that the client is free to ignore it if
        ///it doesn't resize, pick a smaller size (to satisfy aspect ratio or
        ///resize in steps of NxM pixels). If the client picks a smaller size and
        ///is anchored to two opposite anchors (e.g. 'top' and 'bottom'), the
        ///surface will be centered on this axis.
        ///
        ///If the width or height arguments are zero, it means the client should
        ///decide its own window dimension.
        fn configure(&mut self, sender_id: ObjectId, serial: u32, width: u32, height: u32);
        ///surface should be closed
        ///
        ///The closed event is sent by the compositor when the surface will no
        ///longer be shown. The output may have been destroyed or the user may
        ///have asked for it to be removed. Further changes to the surface will be
        ///ignored. The client should destroy the resource after receiving this
        ///event, and create a new surface if they so choose.
        fn closed(&mut self, sender_id: ObjectId);
    }

    pub fn event<T: EvHandler>(state: &mut T, mut wire_msg: WireMsg, payload: WaylandPayload) {
        match wire_msg.op() {
            0 => {
                let serial = wire_msg.next_u32(&payload);
                let width = wire_msg.next_u32(&payload);
                let height = wire_msg.next_u32(&payload);
                state.configure(wire_msg.sender_id(), serial, width, height);
            }
            1 => state.closed(wire_msg.sender_id()),
            e => log::error!("unrecognized event opcode: {e} for interface zwlr_layer_surface_v1"),
        }
    }

    ///Requests for this interface
    pub mod req {
        use super::*;
        ///sets the size of the surface
        ///
        ///Sets the size of the surface in surface-local coordinates. The
        ///compositor will display the surface centered with respect to its
        ///anchors.
        ///
        ///If you pass 0 for either value, the compositor will assign it and
        ///inform you of the assignment in the configure event. You must set your
        ///anchor to opposite edges in the dimensions you omit; not doing so is a
        ///protocol error. Both values are 0 by default.
        ///
        ///Size is double-buffered, see wl_surface.commit.
        pub fn set_size(sender_id: ObjectId, width: u32, height: u32) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 0);
            wire_msg_builder.add_u32(width);
            wire_msg_builder.add_u32(height);
            wire_msg_builder.send()
        }
        ///configures the anchor point of the surface
        ///
        ///Requests that the compositor anchor the surface to the specified edges
        ///and corners. If two orthogonal edges are specified (e.g. 'top' and
        ///'left'), then the anchor point will be the intersection of the edges
        ///(e.g. the top left corner of the output); otherwise the anchor point
        ///will be centered on that edge, or in the center if none is specified.
        ///
        ///Anchor is double-buffered, see wl_surface.commit.
        pub fn set_anchor(sender_id: ObjectId, anchor: u32) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 1);
            wire_msg_builder.add_u32(anchor);
            wire_msg_builder.send()
        }
        ///configures the exclusive geometry of this surface
        ///
        ///Requests that the compositor avoids occluding an area with other
        ///surfaces. The compositor's use of this information is
        ///implementation-dependent - do not assume that this region will not
        ///actually be occluded.
        ///
        ///A positive value is only meaningful if the surface is anchored to one
        ///edge or an edge and both perpendicular edges. If the surface is not
        ///anchored, anchored to only two perpendicular edges (a corner), anchored
        ///to only two parallel edges or anchored to all edges, a positive value
        ///will be treated the same as zero.
        ///
        ///A positive zone is the distance from the edge in surface-local
        ///coordinates to consider exclusive.
        ///
        ///Surfaces that do not wish to have an exclusive zone may instead specify
        ///how they should interact with surfaces that do. If set to zero, the
        ///surface indicates that it would like to be moved to avoid occluding
        ///surfaces with a positive exclusive zone. If set to -1, the surface
        ///indicates that it would not like to be moved to accommodate for other
        ///surfaces, and the compositor should extend it all the way to the edges
        ///it is anchored to.
        ///
        ///For example, a panel might set its exclusive zone to 10, so that
        ///maximized shell surfaces are not shown on top of it. A notification
        ///might set its exclusive zone to 0, so that it is moved to avoid
        ///occluding the panel, but shell surfaces are shown underneath it. A
        ///wallpaper or lock screen might set their exclusive zone to -1, so that
        ///they stretch below or over the panel.
        ///
        ///The default value is 0.
        ///
        ///Exclusive zone is double-buffered, see wl_surface.commit.
        pub fn set_exclusive_zone(sender_id: ObjectId, zone: i32) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 2);
            wire_msg_builder.add_i32(zone);
            wire_msg_builder.send()
        }
        ///sets a margin from the anchor point
        ///
        ///Requests that the surface be placed some distance away from the anchor
        ///point on the output, in surface-local coordinates. Setting this value
        ///for edges you are not anchored to has no effect.
        ///
        ///The exclusive zone includes the margin.
        ///
        ///Margin is double-buffered, see wl_surface.commit.
        pub fn set_margin(
            sender_id: ObjectId,
            top: i32,
            right: i32,
            bottom: i32,
            left: i32,
        ) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 3);
            wire_msg_builder.add_i32(top);
            wire_msg_builder.add_i32(right);
            wire_msg_builder.add_i32(bottom);
            wire_msg_builder.add_i32(left);
            wire_msg_builder.send()
        }
        ///requests keyboard events
        ///
        ///Set how keyboard events are delivered to this surface. By default,
        ///layer shell surfaces do not receive keyboard events; this request can
        ///be used to change this.
        ///
        ///This setting is inherited by child surfaces set by the get_popup
        ///request.
        ///
        ///Layer surfaces receive pointer, touch, and tablet events normally. If
        ///you do not want to receive them, set the input region on your surface
        ///to an empty region.
        ///
        ///Keyboard interactivity is double-buffered, see wl_surface.commit.
        pub fn set_keyboard_interactivity(
            sender_id: ObjectId,
            keyboard_interactivity: u32,
        ) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 4);
            wire_msg_builder.add_u32(keyboard_interactivity);
            wire_msg_builder.send()
        }
        ///assign this layer_surface as an xdg_popup parent
        ///
        ///This assigns an xdg_popup's parent to this layer_surface.  This popup
        ///should have been created via xdg_surface::get_popup with the parent set
        ///to NULL, and this request must be invoked before committing the popup's
        ///initial state.
        ///
        ///See the documentation of xdg_popup for more details about what an
        ///xdg_popup is and how it is used.
        pub fn get_popup(sender_id: ObjectId, popup: ObjectId) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 5);
            wire_msg_builder.add_object(Some(popup));
            wire_msg_builder.send()
        }
        ///ack a configure event
        ///
        ///When a configure event is received, if a client commits the
        ///surface in response to the configure event, then the client
        ///must make an ack_configure request sometime before the commit
        ///request, passing along the serial of the configure event.
        ///
        ///If the client receives multiple configure events before it
        ///can respond to one, it only has to ack the last configure event.
        ///
        ///A client is not required to commit immediately after sending
        ///an ack_configure request - it may even ack_configure several times
        ///before its next surface commit.
        ///
        ///A client may send multiple ack_configure requests before committing, but
        ///only the last request sent before a commit indicates which configure
        ///event the client really is responding to.
        pub fn ack_configure(sender_id: ObjectId, serial: u32) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 6);
            wire_msg_builder.add_u32(serial);
            wire_msg_builder.send()
        }
        ///destroy the layer_surface
        ///
        ///This request destroys the layer surface.
        ///
        ///THIS IS A DESTRUCTOR
        pub fn destroy(sender_id: ObjectId) -> rustix::io::Result<()> {
            let wire_msg_builder = WireMsgBuilder::new(sender_id, 7);
            wire_msg_builder.send()
        }
        ///change the layer of the surface
        ///
        ///Change the layer that the surface is rendered on.
        ///
        ///Layer is double-buffered, see wl_surface.commit.
        pub fn set_layer(sender_id: ObjectId, layer: u32) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 8);
            wire_msg_builder.add_u32(layer);
            wire_msg_builder.send()
        }
        ///set the edge the exclusive zone will be applied to
        ///
        ///Requests an edge for the exclusive zone to apply. The exclusive
        ///edge will be automatically deduced from anchor points when possible,
        ///but when the surface is anchored to a corner, it will be necessary
        ///to set it explicitly to disambiguate, as it is not possible to deduce
        ///which one of the two corner edges should be used.
        ///
        ///The edge must be one the surface is anchored to, otherwise the
        ///invalid_exclusive_edge protocol error will be raised.
        pub fn set_exclusive_edge(sender_id: ObjectId, edge: u32) -> rustix::io::Result<()> {
            let mut wire_msg_builder = WireMsgBuilder::new(sender_id, 9);
            wire_msg_builder.add_u32(edge);
            wire_msg_builder.send()
        }
    }
    ///types of keyboard interaction possible for a layer shell surface
    ///
    ///Types of keyboard interaction possible for layer shell surfaces. The
    ///rationale for this is twofold: (1) some applications are not interested
    ///in keyboard events and not allowing them to be focused can improve the
    ///desktop experience; (2) some applications will want to take exclusive
    ///keyboard focus.
    pub mod keyboard_interactivity {
        ///no keyboard focus is possible
        ///
        ///This value indicates that this surface is not interested in keyboard
        ///events and the compositor should never assign it the keyboard focus.
        ///
        ///This is the default value, set for newly created layer shell surfaces.
        ///
        ///This is useful for e.g. desktop widgets that display information or
        ///only have interaction with non-keyboard input devices.
        pub const NONE: u32 = 0u32;
        ///request exclusive keyboard focus
        ///
        ///Request exclusive keyboard focus if this surface is above the shell surface layer.
        ///
        ///For the top and overlay layers, the seat will always give
        ///exclusive keyboard focus to the top-most layer which has keyboard
        ///interactivity set to exclusive. If this layer contains multiple
        ///surfaces with keyboard interactivity set to exclusive, the compositor
        ///determines the one receiving keyboard events in an implementation-
        ///defined manner. In this case, no guarantee is made when this surface
        ///will receive keyboard focus (if ever).
        ///
        ///For the bottom and background layers, the compositor is allowed to use
        ///normal focus semantics.
        ///
        ///This setting is mainly intended for applications that need to ensure
        ///they receive all keyboard events, such as a lock screen or a password
        ///prompt.
        pub const EXCLUSIVE: u32 = 1u32;
        ///request regular keyboard focus semantics
        ///
        ///This requests the compositor to allow this surface to be focused and
        ///unfocused by the user in an implementation-defined manner. The user
        ///should be able to unfocus this surface even regardless of the layer
        ///it is on.
        ///
        ///Typically, the compositor will want to use its normal mechanism to
        ///manage keyboard focus between layer shell surfaces with this setting
        ///and regular toplevels on the desktop layer (e.g. click to focus).
        ///Nevertheless, it is possible for a compositor to require a special
        ///interaction to focus or unfocus layer shell surfaces (e.g. requiring
        ///a click even if focus follows the mouse normally, or providing a
        ///keybinding to switch focus between layers).
        ///
        ///This setting is mainly intended for desktop shell components (e.g.
        ///panels) that allow keyboard interaction. Using this option can allow
        ///implementing a desktop shell that can be fully usable without the
        ///mouse.
        pub const ON_DEMAND: u32 = 2u32;
    }
    pub mod error {
        ///provided surface state is invalid
        pub const INVALID_SURFACE_STATE: u32 = 0u32;
        ///size is invalid
        pub const INVALID_SIZE: u32 = 1u32;
        ///anchor bitfield is invalid
        pub const INVALID_ANCHOR: u32 = 2u32;
        ///keyboard interactivity is invalid
        pub const INVALID_KEYBOARD_INTERACTIVITY: u32 = 3u32;
        ///exclusive edge is invalid given the surface anchors
        pub const INVALID_EXCLUSIVE_EDGE: u32 = 4u32;
    }
    ///BITFIELD
    pub mod anchor {
        ///the top edge of the anchor rectangle
        pub const TOP: u32 = 1u32;
        ///the bottom edge of the anchor rectangle
        pub const BOTTOM: u32 = 2u32;
        ///the left edge of the anchor rectangle
        pub const LEFT: u32 = 4u32;
        ///the right edge of the anchor rectangle
        pub const RIGHT: u32 = 8u32;
    }
}
