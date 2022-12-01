use log::debug;

use smithay_client_toolkit::reexports::calloop::channel::SyncSender;

use std::{sync::mpsc, thread, time::Duration};

use utils::communication::{Animation, Answer, Img};

mod animations;
use utils::comp_decomp::ReadiedPack;

///The default thread stack size of 2MiB is way too overkill for our purposes
const TSTACK_SIZE: usize = 1 << 18; //256KiB

pub struct Processor {
    frame_sender: SyncSender<(Vec<String>, ReadiedPack)>,
    anim_stoppers: Vec<mpsc::Sender<Vec<String>>>,
}

impl Processor {
    pub fn new(frame_sender: SyncSender<(Vec<String>, ReadiedPack)>) -> Self {
        Self {
            anim_stoppers: Vec::new(),
            frame_sender,
        }
    }

    pub fn process(
        &mut self,
        transition: utils::communication::Transition,
        requests: Vec<(Img, Vec<String>)>,
        old_imgs: Vec<(Box<[u8]>, (u32, u32))>,
    ) -> Answer {
        for ((old_img, dim), (new_img, outputs)) in old_imgs.into_iter().zip(requests) {
            self.transition(
                transition.clone(),
                old_img,
                dim,
                outputs,
                new_img.img.into_boxed_slice(),
            );
        }
        Answer::Ok
    }

    pub fn stop_animations(&mut self, to_stop: &[String]) {
        self.anim_stoppers
            .retain(|a| a.send(to_stop.to_vec()).is_ok());
    }

    fn transition(
        &mut self,
        transition: utils::communication::Transition,
        old_img: Box<[u8]>,
        dim: (u32, u32),
        mut outputs: Vec<String>,
        new_img: Box<[u8]>,
    ) {
        let sender = self.frame_sender.clone();
        let (stopper, stop_recv) = mpsc::channel();
        self.anim_stoppers.push(stopper);
        if let Err(e) = thread::Builder::new()
            .name("animator".to_string()) //Name our threads  for better log messages
            .stack_size(TSTACK_SIZE) //the default of 2MB is way too overkill for this
            .spawn(move || {
                animations::Transition::new(old_img, dim, transition).execute(
                    &new_img,
                    &mut outputs,
                    &sender,
                    &stop_recv,
                );
            })
        {
            log::error!("failed to spawn 'animator' thread: {}", e);
        };
    }
}

impl Drop for Processor {
    //We need to make sure pending animators exited
    fn drop(&mut self) {
        while !self.anim_stoppers.is_empty() {
            self.stop_animations(&Vec::new());
        }
    }
}

///Returns whether the calling function should exit or not
fn send_frame(
    frame: ReadiedPack,
    outputs: &mut Vec<String>,
    timeout: Duration,
    sender: &SyncSender<(Vec<String>, ReadiedPack)>,
    stop_recv: &mpsc::Receiver<Vec<String>>,
) -> bool {
    match stop_recv.recv_timeout(timeout) {
        Ok(to_remove) => {
            outputs.retain(|o| !to_remove.contains(o));
            if outputs.is_empty() || to_remove.is_empty() {
                debug!("STOPPING");
                return true;
            }
        }
        Err(mpsc::RecvTimeoutError::Timeout) => (),
        Err(mpsc::RecvTimeoutError::Disconnected) => return true,
    }
    match sender.send((outputs.clone(), frame)) {
        Ok(()) => false,
        Err(_) => true,
    }
}
