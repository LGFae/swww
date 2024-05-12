use std::{cmp::Reverse, collections::BinaryHeap, num::NonZeroU32};

pub mod bump_pool;
pub mod globals;
pub mod interfaces;
pub mod wire;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObjectId(NonZeroU32);

impl ObjectId {
    #[must_use]
    pub const fn get(&self) -> u32 {
        self.0.get()
    }

    #[must_use]
    pub const fn new(value: NonZeroU32) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn null() -> Option<Self> {
        None
    }
}

#[derive(Clone, Copy, Debug)]
pub enum WlDynObj {
    Output,
    Surface,
    Region,
    LayerSurface,
    Buffer,
    ShmPool,
    Callback,
    Viewport,
    FractionalScale,
    None,
}

pub struct ObjectManager {
    next_id: BinaryHeap<Reverse<u32>>,
    objects: Vec<WlDynObj>,
}

impl ObjectManager {
    pub fn new() -> Self {
        Self {
            next_id: BinaryHeap::new(),
            objects: Vec::new(),
        }
    }
    #[must_use]
    pub fn get(&mut self, object_id: ObjectId) -> WlDynObj {
        let offset = 7 + globals::fractional_scale_support() as u32;
        self.objects[(object_id.get() - offset) as usize]
    }
    /// creates a new Id to use in requests
    #[must_use]
    pub fn create(&mut self, object: WlDynObj) -> ObjectId {
        let offset = 7 + globals::fractional_scale_support() as u32;

        match self.next_id.pop() {
            Some(i) => {
                self.objects[i.0 as usize] = object;
                ObjectId::new(unsafe { NonZeroU32::new(i.0 + offset).unwrap_unchecked() })
            }
            None => {
                let i = self.objects.len() as u32;
                self.objects.push(object);
                ObjectId::new(unsafe { NonZeroU32::new(i + offset).unwrap_unchecked() })
            }
        }
    }

    pub fn remove(&mut self, object_id: ObjectId) {
        let offset = 7 + globals::fractional_scale_support() as u32;
        let pos = object_id.get() - offset;
        self.objects[pos as usize] = WlDynObj::None;
        self.next_id.push(Reverse(pos));
    }
}
