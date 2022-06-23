/// An iterator which iterates two other iterators simultaneously
/// Copy pasted from the Iterator crate, and adapted for our purposes
#[must_use = "iterator adaptors are lazy and do nothing unless consumed"]
pub struct ZipEq<I, J> {
    a: I,
    b: J,
}

pub fn zip_eq<I, J>(i: I, j: J) -> ZipEq<I::IntoIter, J::IntoIter>
where
    I: IntoIterator,
    J: IntoIterator,
{
    ZipEq {
        a: i.into_iter(),
        b: j.into_iter(),
    }
}

impl<I, J> Iterator for ZipEq<I, J>
where
    I: Iterator,
    J: Iterator,
{
    type Item = (I::Item, J::Item);

    fn next(&mut self) -> Option<Self::Item> {
        match (self.a.next(), self.b.next()) {
            (None, None) => None,
            (Some(a), Some(b)) => Some((a, b)),
            _ => unreachable!("Iterators of zip_eq have different sizes!!"),
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
