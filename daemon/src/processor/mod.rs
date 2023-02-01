use log::debug;

use smithay_client_toolkit::reexports::calloop::channel::SyncSender;

use std::sync::{Arc, RwLock};
use std::{sync::mpsc, thread, time::Duration};

use utils::communication::{Answer, Img};

mod animations;
use utils::comp_decomp::ReadiedPack;

///The default thread stack size of 2MiB is way too overkill for our purposes
const TSTACK_SIZE: usize = 1 << 17; //128KiB

pub type ImgWithDim = (Box<[u8]>, (u32, u32));

pub struct Processor {
    frame_sender: SyncSender<(Vec<String>, ReadiedPack)>,
    anim_stoppers: Vec<mpsc::Sender<Vec<String>>>,
    on_going_transitions: Arc<RwLock<Vec<String>>>,
}

impl Processor {
    pub fn new(frame_sender: SyncSender<(Vec<String>, ReadiedPack)>) -> Self {
        Self {
            anim_stoppers: Vec::new(),
            on_going_transitions: Arc::new(RwLock::new(Vec::new())),
            frame_sender,
        }
    }

    pub fn transition(
        &mut self,
        transition: &utils::communication::Transition,
        requests: Vec<(Img, Vec<String>)>,
        old_imgs: Vec<ImgWithDim>,
    ) -> Answer {
        let mut answer = Answer::Ok;
        for ((old_img, dim), (new_img, mut outputs)) in old_imgs.into_iter().zip(requests) {
            self.stop_animations(&outputs);
            let transition = transition.clone();
            let sender = self.frame_sender.clone();
            let (stopper, stop_recv) = mpsc::channel();
            self.anim_stoppers.push(stopper);
            let on_going_transitions = Arc::clone(&self.on_going_transitions);
            if let Err(e) = thread::Builder::new()
                .name("transition".to_string()) //Name our threads  for better log messages
                .stack_size(TSTACK_SIZE) //the default of 2MB is way too overkill for this
                .spawn(move || {
                    on_going_transitions
                        .write()
                        .unwrap()
                        .extend_from_slice(&outputs);
                    animations::Transition::new(old_img, dim, transition).execute(
                        &new_img.img,
                        &mut outputs,
                        &sender,
                        &stop_recv,
                    );
                    on_going_transitions
                        .write()
                        .unwrap()
                        .retain(|output| !outputs.contains(output));
                })
            {
                answer = Answer::Err(format!("failed to spawn transition thread: {e}"));
                log::error!("failed to spawn 'transition' thread: {}", e);
            };
        }
        answer
    }

    pub fn animate(
        &mut self,
        animation: utils::communication::Animation,
        mut outputs: Vec<String>,
        output_size: usize,
    ) -> Answer {
        let mut answer = Answer::Ok;

        let sender = self.frame_sender.clone();
        let (stopper, stop_recv) = mpsc::channel();
        let on_going_transitions = Arc::clone(&self.on_going_transitions);

        self.anim_stoppers.push(stopper);
        if let Err(e) = thread::Builder::new()
            .name("animation".to_string()) //Name our threads  for better log messages
            .stack_size(TSTACK_SIZE) //the default of 2MB is way too overkill for this
            .spawn(move || {
                while on_going_transitions
                    .read()
                    .unwrap()
                    .iter()
                    .any(|output| outputs.contains(output))
                {
                    std::thread::yield_now();
                }
                let mut now = std::time::Instant::now();
                /* We only need to animate if we have > 1 frame */
                if animation.animation.len() == 1 {
                    return;
                }
                for (frame, duration) in animation.animation.iter().cycle() {
                    let frame = frame.ready(output_size);
                    let timeout = duration.saturating_sub(now.elapsed());
                    if send_frame(frame, &mut outputs, timeout, &sender, &stop_recv) {
                        return;
                    }
                    now = std::time::Instant::now();
                }
            })
        {
            answer = Answer::Err(format!("failed to spawn animation thread: {e}"));
            log::error!("failed to spawn 'animation' thread: {e}");
        };

        answer
    }

    pub fn stop_animations(&mut self, to_stop: &[String]) {
        self.on_going_transitions
            .write()
            .unwrap()
            .retain(|output| !to_stop.contains(output));
        self.anim_stoppers
            .retain(|a| a.send(to_stop.to_vec()).is_ok());
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
