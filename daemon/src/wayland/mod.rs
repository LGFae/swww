//! Our own custom wayland implementation
//!
//! The primary reason for doing this is that `wayland-client.rs` offers a very flexible api, at the
//! cost of ergonomics: there are lots of Arcs everywhere, lots of trait implementations with
//! nothing in them, to the point the developers even have macros just to create dummy trait
//! implementations, since it's so annoying.
//!
//! Our own implementation can make several improvements:
//!   * we make all the globals that always in our program exist `const`s, so that they can be
//!   accessed anywhere within the code
//!   * we make the wayland file descriptor a global variable, so it can be accessed anywhere
//!   within the code
//!   * we don't buffer the wayland socket connection, instead just sending the message all at once
//!   every time. This, combined with the two points above, mean we can make request from multiple
//!   threads without having to keep passing weak references to a Backend struct (like how it
//!   happens with `wayland-client.rs`).
//!   * we have a much simpler (from what I can tell), object id manager implementation. That we've
//!   also made global, so it can be called anywhere.
//!
//! Furthermore, this also prevents any changes to `wayland-client.rs` from affecting us. We are
//! now completely independent from them.
use std::num::NonZeroU32;

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
}

/// Object Manager for creating, removing, and maintaining Wayland Objects
pub struct ObjectManager {
    /// stores the object types. The position in this vector + the base offset is the object id
    /// for example, if objects[0] == LayerSurface, then the object of id 0 + BASE_OFFSET = 7 is of
    /// the type "LayerSurface"
    objects: Vec<Option<WlDynObj>>,
    /// the next id we ought to generate
    next: u32,
}

impl ObjectManager {
    /// Ids 1-6, inclusive, are all already taken by the globals in `globals.rs`
    const BASE_OFFSET: u32 = 7;

    pub const fn new() -> Self {
        Self {
            objects: Vec::new(),
            next: 0,
        }
    }

    /// get the type of the wayland object from its id
    ///
    /// panics if the object was already deleted
    #[must_use]
    pub fn get(&self, object_id: ObjectId) -> WlDynObj {
        let offset = Self::BASE_OFFSET + globals::fractional_scale_support() as u32;
        let pos = object_id.get() - offset;
        self.objects[pos as usize].expect("attempting to get a deleted object!")
    }

    /// creates a new Id to use in requests
    #[must_use]
    pub fn create(&mut self, object: WlDynObj) -> ObjectId {
        let offset = Self::BASE_OFFSET + globals::fractional_scale_support() as u32;
        if self.next as usize == self.objects.len() {
            self.next += 1;
            self.objects.push(Some(object));
            ObjectId::new(unsafe { NonZeroU32::new(self.next + offset - 1).unwrap_unchecked() })
        } else {
            let i = self.next as usize;
            self.objects[i] = Some(object);

            // update next to the next available id
            self.next += 1;
            while (self.next as usize) < self.objects.len() {
                if self.objects[self.next as usize].is_none() {
                    break;
                }
                self.next += 1;
            }

            ObjectId::new(unsafe { NonZeroU32::new(i as u32 + offset).unwrap_unchecked() })
        }
    }

    /// removes the wayland object.
    ///
    /// Removing the same element twice currently works just fine and does not panic,
    /// but that may change in the future
    pub fn remove(&mut self, object_id: ObjectId) {
        let offset = Self::BASE_OFFSET + globals::fractional_scale_support() as u32;
        let pos = object_id.get() - offset;
        self.objects[pos as usize] = None;
        if pos < self.next {
            self.next = pos;
        }
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    fn obj_from_u32(u: u32) -> ObjectId {
        ObjectId::new(NonZeroU32::new(u).unwrap())
    }

    #[test]
    fn creating_object_ids() {
        let mut manager = ObjectManager::new();
        let id1 = manager.create(WlDynObj::Region);
        assert_eq!(id1, obj_from_u32(ObjectManager::BASE_OFFSET));
        let id2 = manager.create(WlDynObj::Region);
        assert_eq!(id2, obj_from_u32(ObjectManager::BASE_OFFSET + 1));
        let id3 = manager.create(WlDynObj::Region);
        assert_eq!(id3, obj_from_u32(ObjectManager::BASE_OFFSET + 2));

        manager.remove(id2);
        let id4 = manager.create(WlDynObj::Region);
        assert_eq!(id4, id2);

        manager.remove(id1);
        let id5 = manager.create(WlDynObj::Region);
        assert_eq!(id5, id1);

        manager.remove(id2);
        manager.remove(id1);
        let id6 = manager.create(WlDynObj::Region);
        assert_eq!(id6, id1);

        let id7 = manager.create(WlDynObj::Region);
        assert_eq!(id7, id2);
    }
}
