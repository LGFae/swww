use smithay_client_toolkit::reexports::calloop::channel::SyncSender;
use std::{
    fs::File,
    io::BufReader,
    sync::mpsc,
    time::{Duration, Instant},
};

use keyframe::{ease,mint::Vector2, num_traits::Pow};

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
    pub dimensions: (u32, u32),
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
        pos: (f64, f64),
        bezier: (f64,f64,f64,f64)
    ) -> bool {
        debug!("Starting transition");
        match self.transition_type {
            TransitionType::Fade => self.fade(new_img, outputs, sender, stop_recv),
            TransitionType::Wipe => self.wipe(new_img, outputs, sender, stop_recv,pos.0,bezier),
            TransitionType::Grow => {self.grow(new_img, outputs, sender, stop_recv,pos,bezier)},
            TransitionType::Shrink => self.shrink(new_img, outputs, sender, stop_recv,pos,bezier),
            TransitionType::Random => self.random(new_img, outputs, sender, stop_recv, bezier),
        }
    }

    fn random(
        self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
        bezier: (f64,f64,f64,f64)
    ) -> bool {
        let r: u8 = rand::random();
        let (width, height) = (self.dimensions.0 as usize, self.dimensions.1 as usize);
        match r % 8 {
            0 => self.fade(new_img, outputs, sender, stop_recv),
            1 => self.wipe(new_img, outputs, sender, stop_recv,rand::random::<f64>() % 360.0,bezier),
            5 => {self.grow(new_img, outputs, sender, stop_recv,((rand::random::<usize>() % width) as f64,(rand::random::<usize>() % height) as f64),bezier)},
            6 => {self.grow(new_img, outputs, sender, stop_recv,(width as f64/2.0,height as f64/2.0),bezier)},
            7 => self.shrink(new_img, outputs, sender, stop_recv,(width as f64/2.0,height as f64/2.0),bezier),
            8 => self.shrink(new_img, outputs, sender, stop_recv,((rand::random::<usize>() % width) as f64,(rand::random::<usize>() % height) as f64),bezier),
            _ => unreachable!(),
        }
    }

    fn fade(
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
        angle:f64,
        bezier:(f64,f64,f64,f64)
    ) -> bool {
        let fps = self.fps;
        let speed = self.speed as f64;
        let width = self.dimensions.0 as f64;
        let height = self.dimensions.1 as f64;
        let mut now = Instant::now();
        let center = (width/2.0,height/2.0);
        let screen_diag = ((width.pow(2)+height.pow(2)) as f64).sqrt();

        let mut offset = 0.0;

        let angle = angle.to_radians();
        let circle_radius = screen_diag/2.0;

        // line formula: (x-h)*a + (y-k)*b + C = r^2
        // https://www.desmos.com/calculator/vpvzk12yar
        //
        //checks if a pixel is to the left or right of the line
        let is_low = |pix_x:f64,pix_y:f64,offset:f64,radius: f64| {
            let pix_x = pix_x as f64;  
            let pix_y = pix_y as f64;
            let a = (radius*angle.cos()) as f64;
            let b = (radius*angle.sin()) as f64;
            let offset = offset;
            let x = pix_x-center.0 as f64;
            let y = pix_y-center.1 as f64;
            let res = x*a + y*b + offset;
            if res >= radius.pow(2) {
                true
            } else {
                false
            }
        };

        let curve = keyframe::functions::BezierCurve::from(
            Vector2::from([bezier.0,bezier.1]), Vector2::from([bezier.2,bezier.3])
        );

        let max_offset = circle_radius.pow(2)*2.0;
        let mut time = 0.0;

        loop {
            let transition_img =
                ReadiedPack::new(&mut self.old_img, new_img, |old_pix, new_pix, i| {
                    let pix_x = i as f64 % width;
                    let pix_y = height - i as f64 / width;
                    if is_low(pix_x,pix_y,offset,circle_radius){
                        let step = self.step + ((offset - (i as f64 / width)) / speed) as u8;
                        change_cols(step, old_pix, new_pix);
                    }
                });
            send_transition_frame!(transition_img, outputs, now, fps, sender, stop_recv);
            let value = ease(curve,0.0,max_offset, time);
            time += 1.0/max_offset + (speed as f64)/1000.0;
            now = Instant::now();
            offset = value;
            
        }
    }

    fn grow(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
        pos: (f64, f64),
        bezier:(f64,f64,f64,f64)
    ) -> bool {
        let fps = self.fps;
        let speed = self.speed as usize;
        let (width, height) = (self.dimensions.0 as f64, self.dimensions.1 as f64);
        let (center_x, center_y) = (
            pos.0.min(width - 1.0),
            pos.1.min(height - 1.0),
        );
        let mut now = Instant::now();
        let mut time = 0.0;
        let dist_start = 0.0;
        let mut dist_center = dist_start as usize;
        let dist_end = {
            let mut x = center_x;
            let mut y = center_y;
            if x < width/2.0 {
                x = width - 1.0 - x;
            }
            if y < height/2.0{
                y = height - 1.0 - y;
            }
            ((x.pow(2)+y.pow(2)) as f64).sqrt()
        };
        let curve = keyframe::functions::BezierCurve::from(
            Vector2::from([bezier.0,bezier.1]), Vector2::from([bezier.2,bezier.3])
        );
        loop{
            let transition_img =
            ReadiedPack::new(&mut self.old_img, new_img, |old_pix, new_pix, i| {
                let pix_x = i % width as usize;
                let pix_y = height as usize - i / width as usize;
                let diff_x = pix_x.abs_diff(center_x as usize);
                let diff_y = pix_y.abs_diff(center_y as usize);
                let pix_center_dist = diff_x.pow(2) + diff_y.pow(2);
                if pix_center_dist <= dist_center * dist_center {
                    let step = self
                        .step
                        .saturating_add(((dist_center * dist_center) - pix_center_dist) as u8);
                        change_cols(step, old_pix, new_pix);
                }
            });
            send_transition_frame!(transition_img, outputs, now, fps, sender, stop_recv);
            now = Instant::now();
            let value = ease(curve,dist_start, dist_end, time);
            time += 1.0/dist_end as f64 + (speed as f64)/1000.0;
            dist_center = value as usize;
        }
    }

    fn shrink(
        mut self,
        new_img: &[u8],
        outputs: &mut Vec<String>,
        sender: &SyncSender<(Vec<String>, ReadiedPack)>,
        stop_recv: &mpsc::Receiver<Vec<String>>,
        pos:(f64,f64),
        bezier: (f64,f64,f64,f64)
    ) -> bool {
        let fps = self.fps;
        let speed = self.speed as usize;
        let (width, height) = (self.dimensions.0 as f64, self.dimensions.1 as f64);
        let (center_x, center_y) = (
            pos.0.min(width - 1.0),
            pos.1.min(height - 1.0),
        );
        let mut now = Instant::now();
        let mut time = 0.0;
        let dist_start = {
            let mut x = center_x;
            let mut y = center_y;
            if x < width/2.0 {
                x = width - 1.0 - x;
            }
            if y < height/2.0{
                y = height - 1.0 - y;
            }
            ((x.pow(2)+y.pow(2)) as f64).sqrt()
        };
        let dist_end = 0.0;
        let mut dist_center = dist_start as usize;
        let curve = keyframe::functions::BezierCurve::from(
            Vector2::from([bezier.0,bezier.1]), Vector2::from([bezier.2,bezier.3])
        );
        loop {
            let transition_img =
                ReadiedPack::new(&mut self.old_img, new_img, |old_pix, new_pix, i| {
                    let pix_x = i % width as usize;
                    let pix_y = height as usize - i / width as usize;
                    let diff_x = pix_x.abs_diff(center_x as usize);
                    let diff_y = pix_y.abs_diff(center_y as usize);
                    let pix_center_dist = diff_x.pow(2)+ diff_y.pow(2);
                    if pix_center_dist >= dist_center * dist_center {
                        let step =
                            self.step + (pix_center_dist - (dist_center * dist_center)) as u8;
                        change_cols(step, old_pix, *new_pix);
                    }
                });
            send_transition_frame!(transition_img, outputs, now, fps, sender, stop_recv);
            now = Instant::now();
            if dist_center >= speed {
                let value = ease(curve,dist_start, dist_end, time);
                time += 1.0/dist_start as f64 + (speed as f64)/1000.0;
                dist_center = value as usize;
            } else {
                dist_center = 0;
            }
        }
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
            TT::Wipe,
            TT::Fade,
            TT::Grow,
            TT::Shrink,
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
                std::thread::spawn(move || t.execute(&new_img, &mut dummies, &fr_send, &stop_recv, (0.0,0.0),(0.0,0.0,1.0,1.0)))
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
