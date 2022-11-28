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

use keyframe::{functions::BezierCurve, keyframes, num_traits::Pow, AnimationSequence};

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
    duration: f32,
    step: u8,
    fps: Duration,
    angle: f64,
    pos: (f32, f32),
    bezier: BezierCurve,
}

/// All transitions return whether or not they completed
impl Transition {
    pub fn new(
        old_img: Box<[u8]>,
        dimensions: (u32, u32),
        transition_type: TransitionType,
        duration: f32,
        step: u8,
        fps: Duration,
        angle: f64,
        pos: (f32, f32),
        bezier: BezierCurve,
    ) -> Self {
        Transition {
            old_img,
            dimensions,
            transition_type,
            duration,
            step,
            fps,
            angle,
            pos,
            bezier,
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
            TransitionType::Random => self.random(new_img, outputs, sender, stop_recv),
            TransitionType::Wipe => self.wipe(new_img, outputs, sender, stop_recv),
            TransitionType::Grow => self.grow(new_img, outputs, sender, stop_recv),
            TransitionType::Outer => self.outer(new_img, outputs, sender, stop_recv),
            TransitionType::Center => self.center(new_img, outputs, sender, stop_recv),
            TransitionType::Any => self.any(new_img, outputs, sender, stop_recv),
        }
    }

    fn random(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        let r: u8 = rand::random();
        match r % 7 {
            0 => self.simple(new_img, outputs, sender, stop_recv),
            1 => self.left(new_img, outputs, sender, stop_recv),
            2 => self.right(new_img, outputs, sender, stop_recv),
            3 => self.top(new_img, outputs, sender, stop_recv),
            4 => self.bottom(new_img, outputs, sender, stop_recv),
            5 => {
                self.angle = rand::random::<f64>() % 360.0;
                self.wipe(new_img, outputs, sender, stop_recv)
            }
            6 => self.any(new_img, outputs, sender, stop_recv),
            _ => unreachable!(),
        }
    }

    fn bezier_seq(&self, start: f32, end: f32) -> (AnimationSequence<f32>, Instant) {
        (
            keyframes![(start, 0.0, self.bezier), (end, self.duration, self.bezier)],
            Instant::now(),
        )
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
                    change_cols(self.step, old_pix, *new_pix);
                });
            send_transition_frame!(transition_img, outputs, now, fps, sender, stop_recv);
            now = Instant::now();
        }
    }

    fn wipe(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        let fps = self.fps;
        let width = self.dimensions.0;
        let height = self.dimensions.1;
        let mut now = Instant::now();
        let center = (width / 2, height / 2);
        let screen_diag = ((width.pow(2) + height.pow(2)) as f64).sqrt();

        let circle_radius = screen_diag / 2.0;
        let max_offset = circle_radius.powf(2.0) * 2.0;

        let angle = self.angle.to_radians();

        let mut offset = {
            let (x, y) = angle.sin_cos();
            (x.abs() * width as f64 / 2.0 + y.abs() * height as f64 / 2.0).abs()
        };

        // line formula: (x-h)*a + (y-k)*b + C = r^2
        // https://www.desmos.com/calculator/vpvzk12yar
        //
        // checks if a pixel is to the left or right of the line
        let is_low = |pix_x: f64, pix_y: f64, offset: f64, radius: f64| {
            let a = radius * angle.cos();
            let b = radius * angle.sin();
            let x = pix_x - center.0 as f64;
            let y = pix_y - center.1 as f64;
            let res = x * a + y * b + offset;
            res >= radius.powf(2.0)
        };

        let (mut seq, start) = self.bezier_seq(0.0, max_offset as f32);

        let step = self.step;

        loop {
            let transition_img =
                ReadiedPack::new(&mut self.old_img, new_img, |old_pix, new_pix, i| {
                    let width = width as usize;
                    let height = height as usize;
                    let pix_x = i % width;
                    let pix_y = height - i / width;
                    if is_low(pix_x as f64, pix_y as f64, offset, circle_radius) {
                        change_cols(step, old_pix, *new_pix);
                    }
                });
            send_transition_frame!(transition_img, outputs, now, fps, sender, stop_recv);
            now = Instant::now();

            offset = seq.now() as f64;
            seq.advance_to(start.elapsed().as_secs_f64());
            if start.elapsed().as_secs_f64() >= seq.duration() {
                break;
            }
        }
        self.simple(new_img, outputs, sender, stop_recv)
    }

    fn grow(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        let fps = self.fps;
        let (width, height) = (self.dimensions.0 as f32, self.dimensions.1 as f32);
        let (center_x, center_y) = self.pos;
        let mut dist_center: f32 = 0.0;
        let dist_end: f32 = {
            let mut x = center_x;
            let mut y = center_y;
            if x < width / 2.0 {
                x = width - 1.0 - x;
            }
            if y < height / 2.0 {
                y = height - 1.0 - y;
            }
            ((x.pow(2) + y.pow(2)) as f32).sqrt()
        };
        let mut now = Instant::now();

        let (mut seq, start) = self.bezier_seq(0.0, dist_end);

        loop {
            let transition_img =
                ReadiedPack::new(&mut self.old_img, new_img, |old_pix, new_pix, i| {
                    let (width, height) = (width as usize, height as usize);
                    let pix_x = i % width;
                    let pix_y = height - i / width;
                    let diff_x = pix_x.abs_diff(center_x as usize) as f32;
                    let diff_y = pix_y.abs_diff(center_y as usize) as f32;
                    let pix_center_dist = (diff_x.powf(2.0) + diff_y.powf(2.0)).sqrt();
                    if pix_center_dist <= dist_center {
                        let step = self
                            .step
                            .saturating_add((dist_center - pix_center_dist).log2() as u8);
                        change_cols(step, old_pix, *new_pix);
                    }
                });
            send_transition_frame!(transition_img, outputs, now, fps, sender, stop_recv);
            now = Instant::now();
            dist_center = seq.now();
            seq.advance_to(start.elapsed().as_secs_f64());
            if start.elapsed().as_secs_f64() >= seq.duration() {
                break;
            }
        }
        self.simple(new_img, outputs, sender, stop_recv)
    }

    fn outer(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        let fps = self.fps;
        let (width, height) = (self.dimensions.0 as f32, self.dimensions.1 as f32);
        let (center_x, center_y) = self.pos;
        let mut dist_center = {
            let mut x = center_x;
            let mut y = center_y;
            if x < width / 2.0 {
                x = width - 1.0 - x;
            }
            if y < height / 2.0 {
                y = height - 1.0 - y;
            }
            ((x.pow(2) + y.pow(2)) as f32).sqrt()
        };
        let mut now = Instant::now();

        let (mut seq, start) = self.bezier_seq(dist_center, 0.0);

        loop {
            let transition_img =
                ReadiedPack::new(&mut self.old_img, new_img, |old_pix, new_pix, i| {
                    let (width, height) = (width as usize, height as usize);
                    let pix_x = i % width;
                    let pix_y = height - i / width;
                    let diff_x = pix_x.abs_diff(center_x as usize) as f32;
                    let diff_y = pix_y.abs_diff(center_y as usize) as f32;
                    let pix_center_dist = (diff_x.powf(2.0) + diff_y.powf(2.0)).sqrt();
                    if pix_center_dist >= dist_center {
                        let step = self
                            .step
                            .saturating_add((pix_center_dist - dist_center).log2() as u8);
                        change_cols(step, old_pix, *new_pix);
                    }
                });
            send_transition_frame!(transition_img, outputs, now, fps, sender, stop_recv);
            now = Instant::now();

            dist_center = seq.now();
            seq.advance_to(start.elapsed().as_secs_f64());

            if start.elapsed().as_secs_f64() >= seq.duration() {
                break;
            }
        }
        self.simple(new_img, outputs, sender, stop_recv)
    }

    // aliases

    fn any(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        self.pos = (
            (rand::random::<u32>() % self.dimensions.0) as f32,
            (rand::random::<u32>() % self.dimensions.1) as f32,
        );
        match rand::random::<u8>() % 2 {
            0 => self.grow(new_img, outputs, sender, stop_recv),
            1 => self.outer(new_img, outputs, sender, stop_recv),
            _ => unreachable!(),
        }
    }

    fn center(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        self.pos = (
            (self.dimensions.0 / 2) as f32,
            (self.dimensions.1 / 2) as f32,
        );
        self.grow(new_img, outputs, sender, stop_recv)
    }

    fn right(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        self.angle = 0.0;
        self.wipe(new_img, outputs, sender, stop_recv)
    }

    fn left(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        self.angle = 180.0;
        self.wipe(new_img, outputs, sender, stop_recv)
    }

    fn top(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        self.angle = 90.0;
        self.wipe(new_img, outputs, sender, stop_recv)
    }

    fn bottom(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
    ) -> bool {
        self.angle = 270.0;
        self.wipe(new_img, outputs, sender, stop_recv)
    }
}

fn change_cols(step: u8, old: &mut [u8; 4], new: [u8; 4]) {
    for (old_col, new_col) in old.iter_mut().zip(new) {
        if old_col.abs_diff(new_col) < step {
            *old_col = new_col;
        } else if *old_col > new_col {
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
    pub fn process(self, first_frame: &[u8], fr_sender: &mpsc::Sender<(BitPack, Duration)>) {
        let mut frames = self.gif.into_frames();

        //The first frame should always exist
        let dur_first_frame = frames.next().unwrap().unwrap().delay().numer_denom_ms();
        let dur_first_frame = Duration::from_millis((dur_first_frame.0 / dur_first_frame.1).into());

        let canvas = &mut first_frame.to_owned();
        while let Some(Ok(frame)) = frames.next() {
            let (dur_num, dur_div) = frame.delay().numer_denom_ms();
            let duration = Duration::from_millis((dur_num / dur_div).into());

            // Unwrapping is fine because only the thread will panic in the worst case
            // scenario, not the main loop
            let img = img_resize(frame.into_buffer(), self.dimensions, self.filter).unwrap();

            let pack = BitPack::pack(canvas, &img);
            if fr_sender.send((pack, duration)).is_err() {
                return;
            };
        }
        //Add the first frame we got earlier:
        let _ = fr_sender.send((BitPack::pack(canvas, first_frame), dur_first_frame));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keyframe::mint::Vector2;
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
            2.0,
            100,
            Duration::from_nanos(1),
            0.0,
            (0.0, 0.0),
            BezierCurve::from(Vector2 { x: 1.0, y: 0.0 }, Vector2 { x: 0.0, y: 1.0 }),
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
