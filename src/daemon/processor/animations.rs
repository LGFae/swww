use smithay_client_toolkit::reexports::calloop::channel::SyncSender;
use std::{
    fs::File,
    io::BufReader,
    sync::mpsc,
    time::{Duration, Instant},
};

use fast_image_resize::FilterType;
use image::{codecs::gif::GifDecoder, AnimationDecoder};
use log::debug;

use crate::cli::TransitionType;

use super::{
    comp_decomp::{BitPack, ReadiedPack},
    img_resize, send_frame,
};

pub struct Transition {
    old_img: Box<[u8]>,
    dimensions: (u32, u32),
    transition_type: TransitionType,
    step: u8,
    fps: Duration,
}

/// All transitions return whether or not they completed
impl Transition {
    pub fn new(
        old_img: Box<[u8]>,
        dimensions: (u32, u32),
        transition_type: TransitionType,
        step: u8,
        fps: Duration,
    ) -> Self {
        Transition {
            old_img,
            dimensions,
            transition_type,
            step,
            fps,
        }
    }

    pub fn execute(
        &mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        match self.transition_type {
            TransitionType::Simple => self.simple(new_img, outputs, sender, stop_recv),
            TransitionType::Left => self.left(new_img, outputs, sender, stop_recv),
            TransitionType::Right => self.left(new_img, outputs, sender, stop_recv),
            TransitionType::Top => self.left(new_img, outputs, sender, stop_recv),
            TransitionType::Bottom => self.left(new_img, outputs, sender, stop_recv),
            TransitionType::Center => self.left(new_img, outputs, sender, stop_recv),
            TransitionType::Outer => self.left(new_img, outputs, sender, stop_recv),
            TransitionType::Random => self.left(new_img, outputs, sender, stop_recv),
        }
    }
    fn simple(
        &mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        let mut now = Instant::now();
        loop {
            let transition_img =
                ReadiedPack::new(&mut self.old_img, new_img, |old_pix, new_pix, _| {
                    for (old_col, new_col) in old_pix.iter_mut().zip(new_pix) {
                        if old_col.abs_diff(*new_col) < self.step {
                            *old_col = *new_col;
                        } else if *old_col > *new_col {
                            *old_col -= self.step;
                        } else {
                            *old_col += self.step;
                        }
                    }
                });
            if transition_img.is_empty() {
                debug!("Transition has finished.");
                return true;
            };
            let timeout = self.fps.saturating_sub(now.elapsed());
            if send_frame(transition_img, outputs, timeout, sender, stop_recv) {
                debug!("Transition was interrupted!");
                return false;
            };
            now = Instant::now();
        }
    }

    fn left(
        &mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        let width = self.dimensions.0;
        let mut current_column = 0;
        let mut now = Instant::now();
        loop {
            let transition_img =
                ReadiedPack::new(&mut self.old_img, new_img, |old_pix, new_pix, i| {
                    if i % (width as usize) <= current_column {
                        let step =
                            self.step + ((current_column - (i % (width as usize))) / 10) as u8;
                        for (old_col, new_col) in old_pix.iter_mut().zip(new_pix) {
                            if old_col.abs_diff(*new_col) < step {
                                *old_col = *new_col;
                            } else if *old_col > *new_col {
                                *old_col -= step;
                            } else {
                                *old_col += step;
                            }
                        }
                    }
                });
            if transition_img.is_empty() {
                debug!("Transition has finished.");
                return true;
            };
            let timeout = self.fps.saturating_sub(now.elapsed());
            if send_frame(transition_img, outputs, timeout, sender, stop_recv) {
                debug!("Transition was interrupted!");
                return false;
            };
            now = Instant::now();
            current_column += 10;
        }
    }
}

pub struct GifProcessor {
    gif: GifDecoder<BufReader<File>>,
    dimensions: (u32, u32),
    filter: FilterType,
}

impl GifProcessor {
    pub fn new(
        gif: GifDecoder<BufReader<File>>,
        dimensions: (u32, u32),
        filter: FilterType,
    ) -> Self {
        GifProcessor {
            gif,
            dimensions,
            filter,
        }
    }
    pub fn process(self, first_frame: Box<[u8]>, fr_sender: mpsc::Sender<(BitPack, Duration)>) {
        let mut frames = self.gif.into_frames();

        //The first frame should always exist
        let dur_first_frame = frames.next().unwrap().unwrap().delay().numer_denom_ms();
        let dur_first_frame = Duration::from_millis((dur_first_frame.0 / dur_first_frame.1).into());

        let mut canvas = first_frame.clone();
        while let Some(Ok(frame)) = frames.next() {
            let (dur_num, dur_div) = frame.delay().numer_denom_ms();
            let duration = Duration::from_millis((dur_num / dur_div).into());
            let img = img_resize(frame.into_buffer(), self.dimensions, self.filter);

            let pack = BitPack::pack(&mut canvas, &img);
            if fr_sender.send((pack, duration)).is_err() {
                return;
            };
        }
        //Add the first frame we got earlier:
        let _ = fr_sender.send((BitPack::pack(&mut canvas, &first_frame), dur_first_frame));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay_client_toolkit::reexports::calloop::channel::{self, Channel, SyncSender};

    #[allow(clippy::type_complexity)]
    fn make_senders_and_receivers() -> (
        (
            SyncSender<(Vec<String>, ReadiedPack)>,
            Channel<(Vec<String>, ReadiedPack)>,
        ),
        (mpsc::Sender<Vec<String>>, mpsc::Receiver<Vec<String>>),
    ) {
        (channel::sync_channel(20000), mpsc::channel())
    }

    fn make_test_boxes() -> (Box<[u8]>, Box<[u8]>) {
        let mut vec1 = Vec::with_capacity(4000);
        let mut vec2 = Vec::with_capacity(4000);

        for _ in 0..4000 {
            vec1.push(rand::random());
            vec2.push(rand::random());
        }

        (vec1.into_boxed_slice(), vec2.into_boxed_slice())
    }

    fn test_transition(old_img: Box<[u8]>, transition_type: TransitionType) -> Transition {
        Transition::new(
            old_img,
            (100, 10),
            transition_type,
            1,
            Duration::from_nanos(1),
        )
    }

    fn dummy_outputs() -> Vec<String> {
        vec!["dummy".to_string()]
    }

    #[test]
    fn transitions_should_end_with_equal_vectors() {
        let ((fr_send, _fr_recv), (_stop_send, stop_recv)) = make_senders_and_receivers();

        use TransitionType as TT;
        let transitions = [
            TT::Simple,
            TT::Left,
            TT::Right,
            TT::Bottom,
            TT::Top,
            TT::Center,
            TT::Outer,
            TT::Random,
        ];
        for transition in transitions {
            let (old_img, new_img) = make_test_boxes();
            let mut t = test_transition(old_img, transition.clone());

            assert!(t.execute(&new_img, &mut dummy_outputs(), &fr_send, &stop_recv));
            assert_eq!(
                t.old_img, new_img,
                "Transition {:?} did not end with correct new_img",
                transition
            );
        }
    }
}
