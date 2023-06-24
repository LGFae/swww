use std::{
    marker::PhantomData,
    ptr::NonNull,
    sync::{
        atomic::{self, AtomicUsize, Ordering},
        Condvar, Mutex,
    },
    time::Duration,
};

///This is a barrier that lets us dynamically set the amount of threads that have to wait. We use
///this in order to sync the animations, because outputs may be created or deleted during runtime
///
///It automatically keeps track of how many threads own it
pub struct ArcAnimBarrier {
    ptr: NonNull<AnimBarrier>,
    phantom: PhantomData<AnimBarrier>,
}

impl ArcAnimBarrier {
    pub fn new() -> Self {
        let boxed = Box::new(AnimBarrier {
            rc: AtomicUsize::new(1),
            count: Mutex::new(0),
            cvar: Condvar::new(),
        });
        Self {
            ptr: NonNull::new(Box::into_raw(boxed)).unwrap(),
            phantom: PhantomData,
        }
    }

    pub fn wait(&self, timeout: Duration) {
        self.get().wait(timeout);
    }

    #[inline]
    fn get(&self) -> &AnimBarrier {
        unsafe { self.ptr.as_ref() }
    }
}

impl Clone for ArcAnimBarrier {
    fn clone(&self) -> Self {
        let inner = self.get();

        let _ = inner.rc.fetch_add(1, Ordering::Relaxed);

        Self {
            ptr: self.ptr,
            phantom: PhantomData,
        }
    }
}

impl Drop for ArcAnimBarrier {
    fn drop(&mut self) {
        let inner = self.get();

        if inner.rc.fetch_sub(1, Ordering::Relaxed) != 1 {
            // When someone leaves, increase the counter by 1, since otherwise the other threads
            // might wait forever
            let mut count = inner.count.lock().unwrap();
            *count += 1;
            if inner.rc.load(Ordering::SeqCst) - 1 <= *count {
                *count = 0;
                inner.cvar.notify_all();
            }

            return;
        }

        // This fence is needed to prevent reordering of the use and deletion
        // of the data.
        atomic::fence(Ordering::Acquire);

        let _ = unsafe { Box::from_raw(self.ptr.as_ptr()) };
    }
}

unsafe impl Send for ArcAnimBarrier {}
unsafe impl Sync for ArcAnimBarrier {}

struct AnimBarrier {
    // should ALWAYS be equals to threads
    rc: AtomicUsize,
    count: Mutex<usize>,
    cvar: Condvar,
}

impl AnimBarrier {
    fn wait(&self, timeout: Duration) {
        let mut count = self.count.lock().unwrap();
        *count += 1;

        if self.rc.load(Ordering::SeqCst) - 1 > *count {
            let _ = self
                .cvar
                .wait_timeout_while(count, timeout, |count| *count != 0);
        } else {
            *count = 0;
            self.cvar.notify_all();
        }
    }
}
