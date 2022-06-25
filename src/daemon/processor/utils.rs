/// An iterator which iterates two other iterators simultaneously
/// Copy pasted from the Iterator crate, and adapted for our purposes
#[must_use = "iterator adaptors are lazy and do nothing unless consumed"]
pub struct ZipEq<'a, I> {
    a: std::slice::IterMut<'a, I>,
    b: std::slice::Iter<'a, I>,
}

pub fn zip_eq<'a, I>(i: &'a mut [I], j: &'a [I]) -> ZipEq<'a, I> {
    if i.len() != j.len() {
        unreachable!("Iterators of zip_eq have different sizes!!");
    }
    ZipEq {
        a: i.iter_mut(),
        b: j.iter(),
    }
}

impl<'a, I> Iterator for ZipEq<'a, I> {
    type Item = (&'a mut I, &'a I);

    fn next(&mut self) -> Option<Self::Item> {
        match (self.a.next(), self.b.next()) {
            (None, None) => None,
            (Some(a), Some(b)) => Some((a, b)),
            _ => unsafe { std::hint::unreachable_unchecked() },
        }
    }
}

// The functions bellow were copy pasted and adapted from the bytemuck crate:

#[inline]
pub fn pixels(img: &[u8]) -> &[[u8; 4]] {
    if img.len() % 4 != 0 {
        panic!("Calling pixels with a wrongly formated image");
    }
    unsafe { core::slice::from_raw_parts(img.as_ptr() as *const [u8; 4], img.len() / 4) }
}

#[inline]
pub fn pixels_mut(img: &mut [u8]) -> &mut [[u8; 4]] {
    if img.len() % 4 != 0 {
        panic!("Calling pixels_mut with a wrongly formated image");
    }
    unsafe { core::slice::from_raw_parts_mut(img.as_ptr() as *mut [u8; 4], img.len() / 4) }
}
