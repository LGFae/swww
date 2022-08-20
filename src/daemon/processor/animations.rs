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

macro_rules! send_transition_frame {
    ($img:ident, $outputs:ident, $now:ident, $fps:ident, $sender:ident, $stop_recv:ident) => {
        if $img.is_empty() {
            debug!("Transition has finished.");
            return true;
        }
        let timeout = $fps.saturating_sub($now.elapsed());
        if send_frame($img, $outputs, timeout, $sender, $stop_recv) {
            debug!("Transition was interrupted!");
            return false;
        }
    };
}

pub struct Transition {
    old_img: Box<[u8]>,
    dimensions: (u32, u32),
    transition_type: TransitionType,
    speed: u8,
    step: u8,
    fps: Duration,
}

/// All transitions return whether or not they completed
impl Transition {
    pub fn new(
        old_img: Box<[u8]>,
        dimensions: (u32, u32),
        transition_type: TransitionType,
        speed: u8,
        step: u8,
        fps: Duration,
    ) -> Self {
        Transition {
            old_img,
            dimensions,
            transition_type,
            speed,
            step,
            fps,
        }
    }

    pub fn execute(
        self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        debug!("Starting transition");
        match self.transition_type {
            TransitionType::Simple => self.simple(new_img, outputs, sender, stop_recv),
            TransitionType::Left => self.left(new_img, outputs, sender, stop_recv),
            TransitionType::Right => self.right(new_img, outputs, sender, stop_recv),
            TransitionType::Top => self.top(new_img, outputs, sender, stop_recv),
            TransitionType::Bottom => self.bottom(new_img, outputs, sender, stop_recv),
            TransitionType::Center => self.center(new_img, outputs, sender, stop_recv),
            TransitionType::Outer => self.outer(new_img, outputs, sender, stop_recv),
            TransitionType::Any => self.any(new_img, outputs, sender, stop_recv),
            TransitionType::Random => self.random(new_img, outputs, sender, stop_recv),
        }
    }

    fn random(
        self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        let r: u8 = rand::random();
        match r % 8 {
            0 => self.simple(new_img, outputs, sender, stop_recv),
            1 => self.left(new_img, outputs, sender, stop_recv),
            2 => self.right(new_img, outputs, sender, stop_recv),
            3 => self.top(new_img, outputs, sender, stop_recv),
            4 => self.bottom(new_img, outputs, sender, stop_recv),
            5 => self.center(new_img, outputs, sender, stop_recv),
            6 => self.outer(new_img, outputs, sender, stop_recv),
            7 => self.any(new_img, outputs, sender, stop_recv),
            _ => unreachable!(),
        }
    }

    fn simple(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        let fps = self.fps;
        let mut now = Instant::now();
        loop {
            let transition_img =
                ReadiedPack::new(&mut self.old_img, new_img, |old_pix, new_pix, _| {
                    change_cols(self.step, old_pix, new_pix);
                });
            send_transition_frame!(transition_img, outputs, now, fps, sender, stop_recv);
            now = Instant::now();
        }
    }

    fn left(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        let fps = self.fps;
        let speed = self.speed as usize;
        let width = self.dimensions.0 as usize;
        let mut current_column = 0;
        let mut now = Instant::now();
        loop {
            let transition_img =
                ReadiedPack::new(&mut self.old_img, new_img, |old_pix, new_pix, i| {
                    if i % width <= current_column {
                        let step = self.step + ((current_column - (i % width)) / speed) as u8;
                        change_cols(step, old_pix, new_pix);
                    }
                });
            send_transition_frame!(transition_img, outputs, now, fps, sender, stop_recv);
            now = Instant::now();
            current_column += speed;
        }
    }

    fn right(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        let fps = self.fps;
        let speed = self.speed as usize;
        let width = self.dimensions.0 as usize;
        let mut current_column = width;
        let mut now = Instant::now();
        loop {
            let transition_img =
                ReadiedPack::new(&mut self.old_img, new_img, |old_pix, new_pix, i| {
                    if i % width >= current_column {
                        let step = self.step + (((i % width) - current_column) / speed) as u8;
                        change_cols(step, old_pix, new_pix);
                    }
                });
            send_transition_frame!(transition_img, outputs, now, fps, sender, stop_recv);
            now = Instant::now();
            if current_column >= speed {
                current_column -= speed;
            } else {
                current_column = 0;
            }
        }
    }

    fn top(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        let fps = self.fps;
        let speed = self.speed as usize;
        let width = self.dimensions.0 as usize;
        let mut current_line = 0;
        let mut now = Instant::now();
        loop {
            let transition_img =
                ReadiedPack::new(&mut self.old_img, new_img, |old_pix, new_pix, i| {
                    if i / width <= current_line {
                        let step = self.step + ((current_line - (i / width)) / speed) as u8;
                        change_cols(step, old_pix, new_pix);
                    }
                });
            send_transition_frame!(transition_img, outputs, now, fps, sender, stop_recv);
            now = Instant::now();
            current_line += speed;
        }
    }

    fn bottom(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        let fps = self.fps;
        let speed = self.speed as usize;
        let width = self.dimensions.0 as usize;
        let mut current_line = self.dimensions.1 as usize;
        let mut now = Instant::now();
        loop {
            let transition_img =
                ReadiedPack::new(&mut self.old_img, new_img, |old_pix, new_pix, i| {
                    if i / width >= current_line {
                        let step = self.step + (((i / width) - current_line) / speed) as u8;
                        change_cols(step, old_pix, new_pix);
                    }
                });
            send_transition_frame!(transition_img, outputs, now, fps, sender, stop_recv);
            now = Instant::now();
            if current_line >= speed {
                current_line -= speed;
            } else {
                current_line = 0;
            }
        }
    }

    fn center(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        let fps = self.fps;
        let speed = self.speed as usize;
        let (width, height) = (self.dimensions.0 as usize, self.dimensions.1 as usize);
        let (center_x, center_y) = (width / 2, height / 2);
        let mut dist_center = 0;
        let mut now = Instant::now();
        loop {
            let transition_img =
                ReadiedPack::new(&mut self.old_img, new_img, |old_pix, new_pix, i| {
                    let pix_x = i % width;
                    let pix_y = height - i / width;
                    let diff_x = pix_x.abs_diff(center_x);
                    let diff_y = pix_y.abs_diff(center_y);
                    let pix_center_dist = diff_x * diff_x + diff_y * diff_y;
                    if pix_center_dist <= dist_center * dist_center {
                        let step = self
                            .step
                            .saturating_add(((dist_center * dist_center) - pix_center_dist) as u8);
                        change_cols(step, old_pix, new_pix);
                    }
                });
            send_transition_frame!(transition_img, outputs, now, fps, sender, stop_recv);
            now = Instant::now();
            dist_center += speed;
        }
    }

    fn outer(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        let fps = self.fps;
        let speed = self.speed as usize;
        let (width, height) = (self.dimensions.0 as usize, self.dimensions.1 as usize);
        let (center_x, center_y) = (width / 2, height / 2);
        let mut dist_center = ((center_x * center_x + center_y * center_y) as f64).sqrt() as usize;
        let mut now = Instant::now();
        loop {
            let transition_img =
                ReadiedPack::new(&mut self.old_img, new_img, |old_pix, new_pix, i| {
                    let pix_x = i % width;
                    let pix_y = height - i / width;
                    let diff_x = pix_x.abs_diff(center_x);
                    let diff_y = pix_y.abs_diff(center_y);
                    let pix_center_dist = diff_x * diff_x + diff_y * diff_y;
                    if pix_center_dist >= dist_center * dist_center {
                        let step =
                            self.step + (pix_center_dist - (dist_center * dist_center)) as u8;
                        change_cols(step, old_pix, new_pix);
                    }
                });
            send_transition_frame!(transition_img, outputs, now, fps, sender, stop_recv);
            now = Instant::now();
            if dist_center >= speed {
                dist_center -= speed
            } else {
                dist_center = 0
            }
        }
    }

    fn any(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        let fps = self.fps;
        let speed = self.speed as usize;
        let (width, height) = (self.dimensions.0 as usize, self.dimensions.1 as usize);
        let (center_x, center_y) = (
            rand::random::<usize>() % width,
            rand::random::<usize>() % height,
        );
        let mut dist_center = 0;
        let mut now = Instant::now();
        loop {
            let transition_img =
                ReadiedPack::new(&mut self.old_img, new_img, |old_pix, new_pix, i| {
                    let pix_x = i % width;
                    let pix_y = height - i / width;
                    let diff_x = pix_x.abs_diff(center_x);
                    let diff_y = pix_y.abs_diff(center_y);
                    let pix_center_dist = diff_x * diff_x + diff_y * diff_y;
                    if pix_center_dist <= dist_center * dist_center {
                        let step = self
                            .step
                            .saturating_add(((dist_center * dist_center) - pix_center_dist) as u8);
                        change_cols(step, old_pix, new_pix);
                    }
                });
            send_transition_frame!(transition_img, outputs, now, fps, sender, stop_recv);
            now = Instant::now();
            dist_center += speed;
        }
    }
}

fn change_cols(step: u8, old: &mut [u8; 4], new: &[u8; 4]) {
    for (old_col, new_col) in old.iter_mut().zip(new) {
        if old_col.abs_diff(*new_col) < step {
            *old_col = *new_col;
        } else if *old_col > *new_col {
            *old_col -= step;
        } else {
            *old_col += step;
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

            // Unwrapping is fine because only the thread will panic in the worst case
            // scenario, not the main loop
            let img = img_resize(frame.into_buffer(), self.dimensions, self.filter).unwrap();

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
            100,
            Duration::from_nanos(1),
        )
    }

    fn dummy_outputs() -> Vec<String> {
        vec!["dummy".to_string()]
    }

    #[test]
    fn transitions_should_end_with_equal_vectors() {
        use TransitionType as TT;
        let transitions = [
            TT::Simple,
            TT::Left,
            TT::Right,
            TT::Bottom,
            TT::Top,
            TT::Center,
            TT::Outer,
            TT::Any,
            TT::Random,
        ];
        for transition in transitions {
            let ((fr_send, fr_recv), (_stop_send, stop_recv)) = make_senders_and_receivers();
            let (old_img, new_img) = make_test_boxes();
            let mut transition_img = old_img.clone();
            let t = test_transition(old_img, transition.clone());
            let mut dummies = dummy_outputs();

            let handle = {
                let new_img = new_img.clone();
                std::thread::spawn(move || t.execute(&new_img, &mut dummies, &fr_send, &stop_recv))
            };

            while let Ok((_, i)) = fr_recv.recv() {
                i.unpack(&mut transition_img);
            }

            assert!(handle.join().unwrap_or_else(|_| panic!("{:?}", transition)));
            for (tpix, npix) in transition_img.chunks_exact(4).zip(new_img.chunks_exact(4)) {
                assert_eq!(
                    tpix[0..3],
                    npix[0..3],
                    "Transition {:?} did not end with correct new_img",
                    transition
                );
            }
        }
    }
}
