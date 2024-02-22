use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use rayon::prelude::*;

use log::debug;
use utils::ipc::{ArchivedPosition, ArchivedTransitionType};

use crate::wallpaper::{AnimationToken, Wallpaper};

use keyframe::{
    functions::BezierCurve, keyframes, mint::Vector2, num_traits::Pow, AnimationSequence,
};

macro_rules! change_cols {
    ($step:ident, $old:ident, $new:ident, $done:ident) => {
        for (old_col, new_col) in $old.iter_mut().zip($new) {
            if old_col.abs_diff(*new_col) < $step {
                *old_col = *new_col;
            } else if *old_col > *new_col {
                *old_col -= $step;
                $done = false;
            } else {
                *old_col += $step;
                $done = false;
            }
        }
    };

    ($step:ident, $old:ident, $new:ident) => {
        for (old_col, new_col) in $old.iter_mut().zip($new) {
            if old_col.abs_diff(*new_col) < $step {
                *old_col = *new_col;
            } else if *old_col > *new_col {
                *old_col -= $step;
            } else {
                *old_col += $step;
            }
        }
    };
}

pub(super) struct Transition {
    animation_tokens: Vec<AnimationToken>,
    wallpapers: Vec<Arc<Wallpaper>>,
    dimensions: (u32, u32),
    transition_type: ArchivedTransitionType,
    duration: f32,
    step: u8,
    fps: Duration,
    angle: f64,
    pos: ArchivedPosition,
    bezier: BezierCurve,
    wave: (f32, f32),
    invert_y: bool,
}

/// All transitions return whether or not they completed
impl Transition {
    pub(super) fn new(
        wallpapers: Vec<Arc<Wallpaper>>,
        dimensions: (u32, u32),
        transition: utils::ipc::ArchivedTransition,
    ) -> Self {
        Transition {
            animation_tokens: wallpapers
                .iter()
                .map(|w| w.create_animation_token())
                .collect(),
            wallpapers,
            dimensions,
            transition_type: transition.transition_type,
            duration: transition.duration,
            step: transition.step,
            fps: Duration::from_nanos(1_000_000_000 / transition.fps as u64),
            angle: transition.angle,
            pos: transition.pos,
            bezier: BezierCurve::from(
                Vector2 {
                    x: transition.bezier.0,
                    y: transition.bezier.1,
                },
                Vector2 {
                    x: transition.bezier.2,
                    y: transition.bezier.3,
                },
            ),
            wave: transition.wave,
            invert_y: transition.invert_y,
        }
    }

    pub(super) fn execute(mut self, new_img: &[u8]) {
        debug!("Starting transitions");
        match self.transition_type {
            ArchivedTransitionType::Simple => self.simple(new_img),
            ArchivedTransitionType::Wipe => self.wipe(new_img),
            ArchivedTransitionType::Grow => self.grow(new_img),
            ArchivedTransitionType::Outer => self.outer(new_img),
            ArchivedTransitionType::Wave => self.wave(new_img),
            ArchivedTransitionType::Fade => self.fade(new_img),
        };
        debug!("Transitions finished");
        for (wallpaper, token) in self.wallpapers.iter().zip(self.animation_tokens) {
            token.set_transition_done(wallpaper);
        }
    }

    fn send_frame(&mut self, now: &mut Instant) {
        let fps = self.fps;
        let mut i = 0;
        while i < self.wallpapers.len() {
            let token = &self.animation_tokens[i];
            if !self.wallpapers[i].has_animation_id(token) {
                self.wallpapers.swap_remove(i);
                self.animation_tokens.swap_remove(i);
                continue;
            }
            i += 1;
        }
        let timeout = fps.saturating_sub(now.elapsed());
        spin_sleep::sleep(timeout);
        crate::wake_poll();
        *now = Instant::now();
    }

    fn bezier_seq(&self, start: f32, end: f32) -> (AnimationSequence<f32>, Instant) {
        (
            keyframes![(start, 0.0, self.bezier), (end, self.duration, self.bezier)],
            Instant::now(),
        )
    }

    fn simple(&mut self, new_img: &[u8]) {
        let step = self.step;
        let mut now = Instant::now();
        let mut done = false;
        while !done {
            done = true;
            for wallpaper in self.wallpapers.iter_mut() {
                wallpaper.canvas_change(|canvas| {
                    for (old, new) in canvas.chunks_exact_mut(4).zip(new_img.chunks_exact(3)) {
                        change_cols!(step, old, new, done);
                    }
                });
                wallpaper.draw();
            }
            self.send_frame(&mut now);
        }
    }

    fn fade(&mut self, new_img: &[u8]) {
        let mut step = 0.0;
        let (mut seq, start) = self.bezier_seq(0.0, 1.0);

        let mut now = Instant::now();
        while start.elapsed().as_secs_f64() < seq.duration() {
            for wallpaper in self.wallpapers.iter_mut() {
                wallpaper.canvas_change(|canvas| {
                    canvas
                        .par_chunks_exact_mut(4)
                        .zip(new_img.par_chunks_exact(3))
                        .for_each(|(old_pix, new_pix)| {
                            for (old_col, new_col) in old_pix.iter_mut().zip(new_pix) {
                                *old_col =
                                    (*old_col as f64 * (1.0 - step) + *new_col as f64 * step) as u8;
                            }
                        });
                });
                wallpaper.draw();
            }
            self.send_frame(&mut now);
            step = seq.now() as f64;
            seq.advance_to(start.elapsed().as_secs_f64());
        }
        self.step = 4 + self.step / 4;
        self.simple(new_img)
    }

    fn wave(&mut self, new_img: &[u8]) {
        let width = self.dimensions.0;
        let height = self.dimensions.1;
        let mut now = Instant::now();
        let center = (width / 2, height / 2);
        let screen_diag = ((width.pow(2) + height.pow(2)) as f64).sqrt();

        let angle = self.angle.to_radians();
        let (scale_x, scale_y) = (self.wave.0 as f64, self.wave.1 as f64);

        let circle_radius = screen_diag / 2.0;

        let f = |x: f64| (x / scale_x).sin() * scale_y;

        // graph: https://www.desmos.com/calculator/wunde042es
        //
        // checks if a pixel is to the left or right of the line
        let is_low = |x: f64, y: f64, offset: f64| {
            let x = x - center.0 as f64;
            let y = y - center.1 as f64;

            let lhs = y * angle.cos() - x * angle.sin();
            let rhs = f(x * angle.cos() + y * angle.sin()) + circle_radius - offset;
            lhs >= rhs
        };

        // find the offset to start the transition at
        let mut offset = {
            let mut offset = 0.0;
            for x in 0..width {
                for y in 0..height {
                    if is_low(x as f64, y as f64, offset) {
                        offset += 1.0;
                        break;
                    }
                }
            }
            offset
        };
        let max_offset = 2.0 * circle_radius - offset;
        let (width, height) = (width as usize, height as usize);

        let (mut seq, start) = self.bezier_seq(offset as f32, max_offset as f32);

        let step = self.step;

        while start.elapsed().as_secs_f64() < seq.duration() {
            for wallpaper in self.wallpapers.iter_mut() {
                wallpaper.canvas_change(|canvas| {
                    canvas
                        .par_chunks_exact_mut(4)
                        .zip(new_img.par_chunks_exact(3))
                        .enumerate()
                        .for_each(|(i, (old, new))| {
                            let pix_x = i % width;
                            let pix_y = height - i / width;
                            if is_low(pix_x as f64, pix_y as f64, offset) {
                                change_cols!(step, old, new);
                            }
                        });
                });
                wallpaper.draw();
            }
            self.send_frame(&mut now);

            offset = seq.now() as f64;
            seq.advance_to(start.elapsed().as_secs_f64());
        }
        self.step = 4 + self.step / 4;
        self.simple(new_img)
    }

    fn wipe(&mut self, new_img: &[u8]) {
        let width = self.dimensions.0;
        let height = self.dimensions.1;
        let mut now = Instant::now();
        let center = (width / 2, height / 2);
        let screen_diag = ((width.pow(2) + height.pow(2)) as f64).sqrt();

        let circle_radius = screen_diag / 2.0;
        let max_offset = circle_radius.pow(2) * 2.0;

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
            res >= radius.pow(2)
        };

        let (width, height) = (width as usize, height as usize);
        let (mut seq, start) = self.bezier_seq(0.0, max_offset as f32);

        let step = self.step;

        while start.elapsed().as_secs_f64() < seq.duration() {
            for wallpaper in self.wallpapers.iter_mut() {
                wallpaper.canvas_change(|canvas| {
                    canvas
                        .par_chunks_exact_mut(4)
                        .zip(new_img.par_chunks_exact(3))
                        .enumerate()
                        .for_each(|(i, (old, new))| {
                            let pix_x = i % width;
                            let pix_y = height - i / width;
                            if is_low(pix_x as f64, pix_y as f64, offset, circle_radius) {
                                change_cols!(step, old, new);
                            }
                        });
                });
                wallpaper.draw();
            }
            self.send_frame(&mut now);

            offset = seq.now() as f64;
            seq.advance_to(start.elapsed().as_secs_f64());
        }
        self.step = 4 + self.step / 4;
        self.simple(new_img)
    }

    fn grow(&mut self, new_img: &[u8]) {
        let (width, height) = (self.dimensions.0 as f32, self.dimensions.1 as f32);
        let (center_x, center_y) = self.pos.to_pixel(self.dimensions, self.invert_y);
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
            f32::sqrt(x.pow(2) + y.pow(2))
        };

        let (width, height) = (width as usize, height as usize);
        let (center_x, center_y) = (center_x as usize, center_y as usize);

        let (mut seq, start) = self.bezier_seq(0.0, dist_end);
        let mut now = Instant::now();
        while start.elapsed().as_secs_f64() < seq.duration() {
            for wallpaper in self.wallpapers.iter_mut() {
                wallpaper.canvas_change(|canvas| {
                    canvas
                        .par_chunks_exact_mut(4)
                        .zip(new_img.par_chunks_exact(3))
                        .enumerate()
                        .for_each(|(i, (old, new))| {
                            let pix_x = i % width;
                            let pix_y = height - i / width;
                            let diff_x = pix_x.abs_diff(center_x);
                            let diff_y = pix_y.abs_diff(center_y);
                            let pix_center_dist = f32::sqrt((diff_x.pow(2) + diff_y.pow(2)) as f32);
                            if pix_center_dist <= dist_center {
                                let step = self
                                    .step
                                    .saturating_add((dist_center - pix_center_dist).log2() as u8);
                                change_cols!(step, old, new);
                            }
                        });
                });
                wallpaper.draw();
            }
            self.send_frame(&mut now);

            dist_center = seq.now();
            seq.advance_to(start.elapsed().as_secs_f64());
        }
        self.step = 4 + self.step / 4;
        self.simple(new_img)
    }

    fn outer(&mut self, new_img: &[u8]) {
        let (width, height) = (self.dimensions.0 as f32, self.dimensions.1 as f32);
        let (center_x, center_y) = self.pos.to_pixel(self.dimensions, self.invert_y);
        let mut dist_center = {
            let mut x = center_x;
            let mut y = center_y;
            if x < width / 2.0 {
                x = width - 1.0 - x;
            }
            if y < height / 2.0 {
                y = height - 1.0 - y;
            }
            f32::sqrt(x.pow(2) + y.pow(2))
        };
        let (width, height) = (width as usize, height as usize);
        let (center_x, center_y) = (center_x as usize, center_y as usize);

        let (mut seq, start) = self.bezier_seq(dist_center, 0.0);
        let mut now = Instant::now();
        while start.elapsed().as_secs_f64() < seq.duration() {
            for wallpaper in self.wallpapers.iter_mut() {
                wallpaper.canvas_change(|canvas| {
                    canvas
                        .par_chunks_exact_mut(4)
                        .zip(new_img.par_chunks_exact(3))
                        .enumerate()
                        .for_each(|(i, (old, new))| {
                            let pix_x = i % width;
                            let pix_y = height - i / width;
                            let diff_x = pix_x.abs_diff(center_x);
                            let diff_y = pix_y.abs_diff(center_y);
                            let pix_center_dist = f32::sqrt((diff_x.pow(2) + diff_y.pow(2)) as f32);
                            if pix_center_dist >= dist_center {
                                let step = self
                                    .step
                                    .saturating_add((pix_center_dist - dist_center).log2() as u8);
                                change_cols!(step, old, new);
                            }
                        });
                });
                wallpaper.draw();
            }
            self.send_frame(&mut now);

            dist_center = seq.now();
            seq.advance_to(start.elapsed().as_secs_f64());
        }
        self.step = 4 + self.step / 4;
        self.simple(new_img)
    }
}
