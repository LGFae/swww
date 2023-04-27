use std::{
    sync::{Condvar, Mutex, RwLock},
    time::Duration,
};

///This is a barrier that lets us dynamically set the amount of threads that have to wait. We use
///this in order to sync the animations, because outputs may be created or deleted during runtime
pub struct SyncBarrier {
    goal: RwLock<u8>,
    cur: Mutex<u8>,
    condvar: Condvar,
}

impl SyncBarrier {
    pub fn new(goal: u8) -> Self {
        Self {
            goal: RwLock::new(goal),
            cur: Mutex::new(0),
            condvar: Condvar::new(),
        }
    }

    pub fn set_goal(&self, new_goal: u8) {
        let mut goal = self.goal.write().unwrap();
        *goal = new_goal;
    }

    pub fn inc_and_wait(&self, timeout: Duration) {
        let mut cur = self.cur.lock().unwrap();
        let goal = self.goal.read().unwrap();

        *cur += 1;
        if *cur != *goal {
            drop(goal);
            while *cur != 0 {
                cur = self.condvar.wait_timeout(cur, timeout).unwrap().0;
            }
        } else {
            *cur = 0;
            self.condvar.notify_all();
        }
    }
}
