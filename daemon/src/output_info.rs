use common::ipc::Scale;
use waybackend::{Waybackend, objman::ObjectManager, types::ObjectId};

use std::num::NonZeroI32;

use crate::WaylandObject;

pub struct OutputInfo {
    pub name: Option<Box<str>>,
    pub desc: Option<Box<str>>,
    pub scale_factor: Scale,

    pub output: ObjectId,
    pub output_name: u32,
}

impl OutputInfo {
    pub fn new(
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        registry: ObjectId,
        output_name: u32,
    ) -> Self {
        let output = objman.create(WaylandObject::Output);
        crate::wayland::wl_registry::req::bind(
            backend,
            registry,
            output_name,
            output,
            "wl_output",
            4,
        )
        .unwrap();
        Self {
            name: None,
            desc: None,
            scale_factor: Scale::Output(NonZeroI32::new(1).unwrap()),
            output,
            output_name,
        }
    }
}
