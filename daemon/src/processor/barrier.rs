use std::{
    sync::{Condvar, Mutex, RwLock},
    time::Duration,
};

pub struct CountingBarrier {
    goal: RwLock<u8>,
    cur: Mutex<u8>,
    condvar: Condvar,
}

impl CountingBarrier {
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

    pub fn inc_and_wait_while<F>(&self, timeout: Duration, mut f: F)
    where
        F: FnMut() -> bool,
    {
        let mut cur = self.cur.lock().unwrap();
        let goal = self.goal.read().unwrap();

        *cur += 1;
        if *cur != *goal {
            drop(goal);
            while *cur != 0 {
                if f() {
                    return;
                }
                cur = self.condvar.wait_timeout(cur, timeout).unwrap().0;
            }
        } else {
            *cur = 0;
            self.condvar.notify_all();
        }
    }
}
