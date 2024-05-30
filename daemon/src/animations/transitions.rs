use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use log::debug;
use utils::ipc::{Position, TransitionType};

use crate::{
    wallpaper::{AnimationToken, Wallpaper},
    wayland::globals,
};

use keyframe::{
    functions::BezierCurve, keyframes, mint::Vector2, num_traits::Pow, AnimationSequence,
};

pub(super) struct Transition<'a> {
    animation_tokens: Vec<AnimationToken>,
    wallpapers: &'a mut Vec<Arc<Wallpaper>>,
    dimensions: (u32, u32),
    transition_type: TransitionType,
    duration: f32,
    step: u8,
    fps: Duration,
    angle: f64,
    pos: Position,
    bezier: BezierCurve,
    wave: (f32, f32),
    invert_y: bool,
}

/// All transitions return whether or not they completed
impl<'a> Transition<'a> {
    pub(super) fn new(
        wallpapers: &'a mut Vec<Arc<Wallpaper>>,
        dimensions: (u32, u32),
        transition: &utils::ipc::Transition,
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
            step: transition.step.get(),
            fps: Duration::from_nanos(1_000_000_000 / transition.fps as u64),
            angle: transition.angle,
            pos: transition.pos.clone(),
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
            TransitionType::None => self.none(new_img),
            TransitionType::Simple => self.simple(new_img),
            TransitionType::Wipe => self.wipe(new_img),
            TransitionType::Grow => self.grow(new_img),
            TransitionType::Outer => self.outer(new_img),
            TransitionType::Wave => self.wave(new_img),
            TransitionType::Fade => self.fade(new_img),
        };
        debug!("Transitions finished");
    }

    fn updt_wallpapers(&mut self, now: &mut Instant) {
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
        crate::wallpaper::attach_buffers_and_damange_surfaces(self.wallpapers);
        let timeout = self.fps.saturating_sub(now.elapsed());
        crate::spin_sleep(timeout);
        crate::wallpaper::commit_wallpapers(self.wallpapers);
        *now = Instant::now();
    }

    fn bezier_seq(&self, start: f32, end: f32) -> (AnimationSequence<f32>, Instant) {
        (
            keyframes![(start, 0.0, self.bezier), (end, self.duration, self.bezier)],
            Instant::now(),
        )
    }

    fn none(&mut self, new: &[u8]) {
        self.wallpapers
            .iter()
            .for_each(|w| w.canvas_change(|canvas| canvas.copy_from_slice(new)));
        crate::wallpaper::attach_buffers_and_damange_surfaces(self.wallpapers);
        crate::wallpaper::commit_wallpapers(self.wallpapers);
    }

    fn simple(&mut self, new_img: &[u8]) {
        let step = self.step;
        let mut now = Instant::now();
        let mut done = false;
        while !done {
            done = true;
            for wallpaper in self.wallpapers.iter() {
                wallpaper.canvas_change(|canvas| {
                    for (old, new) in canvas.iter_mut().zip(new_img) {
                        change_byte(step, old, new);
                    }
                    done = canvas == new_img;
                });
            }
            self.updt_wallpapers(&mut now);
        }
    }

    fn fade(&mut self, new_img: &[u8]) {
        let mut step = 0;
        let (mut seq, start) = self.bezier_seq(0.0, 1.0);

        let mut now = Instant::now();
        while start.elapsed().as_secs_f64() < seq.duration() {
            for wallpaper in self.wallpapers.iter() {
                wallpaper.canvas_change(|canvas| {
                    for (old, new) in canvas.iter_mut().zip(new_img) {
                        let x = *old as u16 * (256 - step);
                        let y = *new as u16 * step;
                        *old = ((x + y) >> 8) as u8;
                    }
                });
            }
            self.updt_wallpapers(&mut now);
            step = (256.0 * seq.now() as f64).trunc() as u16;
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
        let (sin, cos) = angle.sin_cos();
        let (scale_x, scale_y) = (self.wave.0 as f64, self.wave.1 as f64);

        let circle_radius = screen_diag / 2.0;

        // graph: https://www.desmos.com/calculator/wunde042es
        //
        // checks if a pixel is to the left or right of the line
        let is_low = |x: f64, y: f64, offset: f64| {
            let x = x - center.0 as f64;
            let y = y - center.1 as f64;

            let lhs = y * sin - x * cos;

            let f = ((x * sin + y * cos) / scale_x).sin() * scale_y;
            let rhs = f - circle_radius + offset / circle_radius;
            lhs <= rhs
        };

        let mut offset = (sin.abs() * width as f64 + cos.abs() * height as f64) * 2.0;
        let a = circle_radius * cos;
        let b = circle_radius * sin;
        let max_offset = circle_radius.pow(2) * 2.0;
        let (width, height) = (width as usize, height as usize);

        let (mut seq, start) = self.bezier_seq(offset as f32, max_offset as f32);

        let step = self.step;
        let channels = globals::pixel_format().channels() as usize;
        let stride = width * channels;
        while start.elapsed().as_secs_f64() < seq.duration() {
            offset = seq.now() as f64;
            seq.advance_to(start.elapsed().as_secs_f64());

            for wallpaper in self.wallpapers.iter() {
                wallpaper.canvas_change(|canvas| {
                    // divide in 3 sections: the one we know will not be drawn to, the one we know
                    // WILL be drawn to, and the one we need to do a more expensive check on.
                    // We do this by creating 2 lines: the first tangential to the wave's peaks,
                    // the second to its valeys. In-between is where we have to do the more
                    // expensive checks
                    for line in 0..height {
                        let y = ((height - line) as f64 - center.1 as f64 - scale_y * sin) * b;
                        let x = (circle_radius.powi(2) - y - offset) / a
                            + center.0 as f64
                            + scale_y * cos;
                        let x = x.min(width as f64);
                        let (col_begin, col_end) = if a.is_sign_negative() {
                            (0usize, x as usize * channels)
                        } else {
                            (x as usize * channels, stride)
                        };
                        for col in col_begin..col_end {
                            let old = unsafe { canvas.get_unchecked_mut(line * stride + col) };
                            let new = unsafe { new_img.get_unchecked(line * stride + col) };
                            change_byte(step, old, new);
                        }
                        let old_x = x;
                        let y = ((height - line) as f64 - center.1 as f64 + scale_y * sin) * b;
                        let x = (circle_radius.powi(2) - y - offset) / a + center.0 as f64
                            - scale_y * cos;
                        let x = x.min(width as f64);
                        let (col_begin, col_end) = if old_x < x {
                            (old_x as usize, x as usize)
                        } else {
                            (x as usize, old_x as usize)
                        };
                        for col in col_begin..col_end {
                            if is_low(col as f64, line as f64, offset) {
                                let i = line * stride + col * channels;
                                for j in 0..channels {
                                    let old = unsafe { canvas.get_unchecked_mut(i + j) };
                                    let new = unsafe { new_img.get_unchecked(i + j) };
                                    change_byte(step, old, new);
                                }
                            }
                        }
                    }
                });
            }

            self.updt_wallpapers(&mut now);
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
            (x.abs() * width as f64 + y.abs() * height as f64) * 2.0
        };

        let a = circle_radius * angle.cos();
        let b = circle_radius * angle.sin();

        let (width, height) = (width as usize, height as usize);
        let (mut seq, start) = self.bezier_seq(offset as f32, max_offset as f32);

        let step = self.step;
        let channels = globals::pixel_format().channels() as usize;
        let stride = width * channels;
        while start.elapsed().as_secs_f64() < seq.duration() {
            offset = seq.now() as f64;
            seq.advance_to(start.elapsed().as_secs_f64());
            for wallpaper in self.wallpapers.iter() {
                wallpaper.canvas_change(|canvas| {
                    // line formula: (x-h)*a + (y-k)*b + C = r^2
                    // https://www.desmos.com/calculator/vpvzk12yar
                    for line in 0..height {
                        let y = ((height - line) as f64 - center.1 as f64) * b;
                        let x = (circle_radius.powi(2) - y - offset) / a + center.0 as f64;
                        let x = x.min(width as f64);
                        let (col_begin, col_end) = if a.is_sign_negative() {
                            (0usize, x as usize * channels)
                        } else {
                            (x as usize * channels, stride)
                        };
                        for col in col_begin..col_end {
                            let old = unsafe { canvas.get_unchecked_mut(line * stride + col) };
                            let new = unsafe { new_img.get_unchecked(line * stride + col) };
                            change_byte(step, old, new);
                        }
                    }
                });
            }
            self.updt_wallpapers(&mut now);
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

        let step = self.step;
        let channels = globals::pixel_format().channels() as usize;
        let stride = width * channels;
        let (mut seq, start) = self.bezier_seq(0.0, dist_end);
        let mut now = Instant::now();
        while start.elapsed().as_secs_f64() < seq.duration() {
            for wallpaper in self.wallpapers.iter() {
                wallpaper.canvas_change(|canvas| {
                    let line_begin = center_y.saturating_sub(dist_center as usize);
                    let line_end = height.min(center_y + dist_center as usize);

                    // to plot half a circle with radius r, we do sqrt(r^2 - x^2)
                    for line in line_begin..line_end {
                        let offset = (dist_center.powi(2) - (center_y as f32 - line as f32).powi(2))
                            .sqrt() as usize;
                        let col_begin = center_x.saturating_sub(offset) * channels;
                        let col_end = width.min(center_x + offset) * channels;
                        for col in col_begin..col_end {
                            let old = unsafe { canvas.get_unchecked_mut(line * stride + col) };
                            let new = unsafe { new_img.get_unchecked(line * stride + col) };
                            change_byte(step, old, new);
                        }
                    }
                });
            }
            self.updt_wallpapers(&mut now);

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

        let step = self.step;
        let channels = globals::pixel_format().channels() as usize;
        let stride = width * channels;
        let (mut seq, start) = self.bezier_seq(dist_center, 0.0);
        let mut now = Instant::now();
        while start.elapsed().as_secs_f64() < seq.duration() {
            for wallpaper in self.wallpapers.iter() {
                wallpaper.canvas_change(|canvas| {
                    // to plot half a circle with radius r, we do sqrt(r^2 - x^2)
                    for line in 0..height {
                        let offset = (dist_center.powi(2) - (center_y as f32 - line as f32).powi(2))
                            .sqrt() as usize;
                        let col_begin = center_x.saturating_sub(offset) * channels;
                        let col_end = width.min(center_x + offset) * channels;
                        for col in 0..col_begin {
                            let old = unsafe { canvas.get_unchecked_mut(line * stride + col) };
                            let new = unsafe { new_img.get_unchecked(line * stride + col) };
                            change_byte(step, old, new);
                        }
                        for col in col_end..stride {
                            let old = unsafe { canvas.get_unchecked_mut(line * stride + col) };
                            let new = unsafe { new_img.get_unchecked(line * stride + col) };
                            change_byte(step, old, new);
                        }
                    }
                });
            }
            self.updt_wallpapers(&mut now);

            dist_center = seq.now();
            seq.advance_to(start.elapsed().as_secs_f64());
        }
        self.step = 4 + self.step / 4;
        self.simple(new_img)
    }
}

#[inline(always)]
fn change_byte(step: u8, old: &mut u8, new: &u8) {
    if old.abs_diff(*new) < step {
        *old = *new;
    } else if *old > *new {
        *old -= step;
    } else {
        *old += step;
    }
}
