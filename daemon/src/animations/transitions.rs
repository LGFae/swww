use std::{cell::RefCell, rc::Rc, time::Instant};

use crate::{WaylandObject, wallpaper::Wallpaper};
use common::ipc::{PixelFormat, Transition, TransitionType};

use keyframe::{
    AnimationSequence, functions::BezierCurve, keyframes, mint::Vector2, num_traits::Pow,
};
use waybackend::{Waybackend, objman::ObjectManager};

fn bezier_seq(transition: &Transition, start: f32, end: f32) -> (AnimationSequence<f32>, Instant) {
    let bezier = BezierCurve::from(
        Vector2 {
            x: transition.bezier.0,
            y: transition.bezier.1,
        },
        Vector2 {
            x: transition.bezier.2,
            y: transition.bezier.3,
        },
    );
    (
        keyframes![(start, 0.0, bezier), (end, transition.duration, bezier)],
        Instant::now(),
    )
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

struct None;

impl None {
    fn new() -> Self {
        Self
    }

    fn run(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        pixel_format: PixelFormat,
        wallpapers: &mut [Rc<RefCell<Wallpaper>>],
        img: &[u8],
    ) -> bool {
        wallpapers.iter().for_each(|w| {
            w.borrow_mut()
                .canvas_change(backend, objman, pixel_format, |canvas| {
                    canvas.copy_from_slice(img)
                })
        });
        true
    }
}

#[allow(private_interfaces)]
pub enum Effect {
    None(None),
    Simple(Simple),
    Fade(Fade),
    Wave(Wave),
    Wipe(Wipe),
    Grow(Grow),
    Outer(Outer),
}

impl Effect {
    pub fn new(transition: &Transition, pixel_format: PixelFormat, dimensions: (u32, u32)) -> Self {
        match transition.transition_type {
            TransitionType::Simple => Self::Simple(Simple::new(transition.step.get())),
            TransitionType::Fade => Self::Fade(Fade::new(transition)),
            TransitionType::Outer => Self::Outer(Outer::new(transition, pixel_format, dimensions)),
            TransitionType::Wipe => Self::Wipe(Wipe::new(transition, pixel_format, dimensions)),
            TransitionType::Grow => Self::Grow(Grow::new(transition, pixel_format, dimensions)),
            TransitionType::Wave => Self::Wave(Wave::new(transition, pixel_format, dimensions)),
            TransitionType::None => Self::None(None::new()),
        }
    }

    pub fn execute(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        pixel_format: PixelFormat,
        wallpapers: &mut [Rc<RefCell<Wallpaper>>],
        img: &[u8],
    ) -> bool {
        let done = match self {
            Effect::None(effect) => effect.run(backend, objman, pixel_format, wallpapers, img),
            Effect::Simple(effect) => effect.run(backend, objman, pixel_format, wallpapers, img),
            Effect::Fade(effect) => effect.run(backend, objman, pixel_format, wallpapers, img),
            Effect::Wave(effect) => effect.run(backend, objman, pixel_format, wallpapers, img),
            Effect::Wipe(effect) => effect.run(backend, objman, pixel_format, wallpapers, img),
            Effect::Grow(effect) => effect.run(backend, objman, pixel_format, wallpapers, img),
            Effect::Outer(effect) => effect.run(backend, objman, pixel_format, wallpapers, img),
        };
        // we only finish for real if we are doing a None or a Simple transition
        if done {
            *self = match self {
                Effect::None(_) | Effect::Simple(_) => return true,
                Effect::Fade(t) => Effect::Simple(Simple::new((t.step / 4 + 4) as u8)),
                Effect::Wave(t) => Effect::Simple(Simple::new(t.step / 4 + 4)),
                Effect::Wipe(t) => Effect::Simple(Simple::new(t.step / 4 + 4)),
                Effect::Grow(t) => Effect::Simple(Simple::new(t.step / 4 + 4)),
                Effect::Outer(t) => Effect::Simple(Simple::new(t.step / 4 + 4)),
            };
            return false;
        }
        done
    }
}

struct Simple {
    step: u8,
}

impl Simple {
    fn new(step: u8) -> Self {
        Self { step }
    }
    fn run(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        pixel_format: PixelFormat,
        wallpapers: &mut [Rc<RefCell<Wallpaper>>],
        img: &[u8],
    ) -> bool {
        let step = self.step;
        let mut done = true;
        for wallpaper in wallpapers.iter() {
            wallpaper
                .borrow_mut()
                .canvas_change(backend, objman, pixel_format, |canvas| {
                    for (old, new) in canvas.iter_mut().zip(img) {
                        change_byte(step, old, new);
                    }
                    done = done && canvas == img;
                });
        }
        done
    }
}

struct Fade {
    start: Instant,
    seq: AnimationSequence<f32>,
    step: u16,
}

impl Fade {
    fn new(transition: &Transition) -> Self {
        let (seq, start) = bezier_seq(transition, 0.0, 1.0);
        let step = 0;
        Self { start, seq, step }
    }
    fn run(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        pixel_format: PixelFormat,
        wallpapers: &mut [Rc<RefCell<Wallpaper>>],
        img: &[u8],
    ) -> bool {
        for wallpaper in wallpapers.iter() {
            wallpaper
                .borrow_mut()
                .canvas_change(backend, objman, pixel_format, |canvas| {
                    for (old, new) in canvas.iter_mut().zip(img) {
                        let x = *old as u16 * (256 - self.step);
                        let y = *new as u16 * self.step;
                        *old = ((x + y) >> 8) as u8;
                    }
                });
        }
        self.step = (256.0 * self.seq.now() as f64).trunc() as u16;
        self.seq.advance_to(self.start.elapsed().as_secs_f64());
        self.start.elapsed().as_secs_f64() > self.seq.duration()
    }
}

struct Wave {
    start: Instant,
    seq: AnimationSequence<f32>,
    width: usize,
    height: usize,
    center: (u32, u32),
    stride: usize,
    sin: f64,
    cos: f64,
    scale_x: f64,
    scale_y: f64,
    circle_radius: f64,
    a: f64,
    b: f64,
    step: u8,
}

impl Wave {
    fn new(transition: &Transition, pixel_format: PixelFormat, dimensions: (u32, u32)) -> Self {
        let width = dimensions.0;
        let height = dimensions.1;
        let center = (width / 2, height / 2);
        let screen_diag = ((width.pow(2) + height.pow(2)) as f64).sqrt();

        let angle = transition.angle.to_radians();
        let (sin, cos) = angle.sin_cos();
        let (scale_x, scale_y) = (transition.wave.0 as f64, transition.wave.1 as f64);

        let circle_radius = screen_diag / 2.0;

        let offset = (sin.abs() * width as f64 + cos.abs() * height as f64) * 2.0;
        let a = circle_radius * cos;
        let b = circle_radius * sin;
        let max_offset = circle_radius.pow(2) * 2.0;
        let (width, height) = (width as usize, height as usize);

        let (seq, start) = bezier_seq(transition, offset as f32, max_offset as f32);

        let step = transition.step.get();
        let channels = pixel_format.channels() as usize;
        let stride = width * channels;
        Self {
            start,
            seq,
            width,
            height,
            center,
            stride,
            a,
            b,
            sin,
            cos,
            scale_x,
            scale_y,
            circle_radius,
            step,
        }
    }
    fn run(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        pixel_format: PixelFormat,
        wallpapers: &mut [Rc<RefCell<Wallpaper>>],
        img: &[u8],
    ) -> bool {
        let Self {
            width,
            height,
            center,
            stride,
            sin,
            cos,
            scale_x,
            scale_y,
            circle_radius,
            a,
            b,
            step,
            ..
        } = *self;
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

        let channels = pixel_format.channels() as usize;
        let offset = self.seq.now() as f64;
        self.seq.advance_to(self.start.elapsed().as_secs_f64());

        for wallpaper in wallpapers.iter() {
            wallpaper
                .borrow_mut()
                .canvas_change(backend, objman, pixel_format, |canvas| {
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
                            let new = unsafe { img.get_unchecked(line * stride + col) };
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
                                    let new = unsafe { img.get_unchecked(i + j) };
                                    change_byte(step, old, new);
                                }
                            }
                        }
                    }
                });
        }

        self.start.elapsed().as_secs_f64() > self.seq.duration()
    }
}

struct Wipe {
    start: Instant,
    seq: AnimationSequence<f32>,
    width: usize,
    height: usize,
    center: (u32, u32),
    stride: usize,
    circle_radius: f64,
    a: f64,
    b: f64,
    step: u8,
}

impl Wipe {
    fn new(transition: &Transition, pixel_format: PixelFormat, dimensions: (u32, u32)) -> Self {
        let width = dimensions.0;
        let height = dimensions.1;
        let center = (width / 2, height / 2);
        let screen_diag = ((width.pow(2) + height.pow(2)) as f64).sqrt();

        let circle_radius = screen_diag / 2.0;
        let max_offset = circle_radius.pow(2) * 2.0;

        let angle = transition.angle.to_radians();

        let offset = {
            let (x, y) = angle.sin_cos();
            (x.abs() * width as f64 + y.abs() * height as f64) * 2.0
        };

        let a = circle_radius * angle.cos();
        let b = circle_radius * angle.sin();

        let (width, height) = (width as usize, height as usize);
        let (seq, start) = bezier_seq(transition, offset as f32, max_offset as f32);

        let step = transition.step.get();
        let channels = pixel_format.channels() as usize;
        let stride = width * channels;
        Self {
            start,
            seq,
            width,
            height,
            center,
            stride,
            circle_radius,
            a,
            b,
            step,
        }
    }
    fn run(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        pixel_format: PixelFormat,
        wallpapers: &mut [Rc<RefCell<Wallpaper>>],
        img: &[u8],
    ) -> bool {
        let Self {
            width,
            height,
            center,
            stride,
            circle_radius,
            a,
            b,
            step,
            ..
        } = *self;
        let channels = pixel_format.channels() as usize;
        let offset = self.seq.now() as f64;
        self.seq.advance_to(self.start.elapsed().as_secs_f64());
        for wallpaper in wallpapers.iter() {
            wallpaper
                .borrow_mut()
                .canvas_change(backend, objman, pixel_format, |canvas| {
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
                            let new = unsafe { img.get_unchecked(line * stride + col) };
                            change_byte(step, old, new);
                        }
                    }
                });
        }
        self.start.elapsed().as_secs_f64() > self.seq.duration()
    }
}

struct Grow {
    start: Instant,
    seq: AnimationSequence<f32>,
    width: usize,
    height: usize,
    center_x: usize,
    center_y: usize,
    stride: usize,
    dist_center: f32,
    step: u8,
}

impl Grow {
    fn new(transition: &Transition, pixel_format: PixelFormat, dimensions: (u32, u32)) -> Self {
        let (width, height) = (dimensions.0 as f32, dimensions.1 as f32);
        let (center_x, center_y) = transition.pos.to_pixel(dimensions, transition.invert_y);
        let dist_center: f32 = 0.0;
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

        let step = transition.step.get();
        let channels = pixel_format.channels() as usize;
        let stride = width * channels;
        let (seq, start) = bezier_seq(transition, 0.0, dist_end);
        Self {
            start,
            seq,
            width,
            height,
            center_x,
            center_y,
            stride,
            dist_center,
            step,
        }
    }
    fn run(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        pixel_format: PixelFormat,
        wallpapers: &mut [Rc<RefCell<Wallpaper>>],
        img: &[u8],
    ) -> bool {
        let Self {
            width,
            height,
            center_x,
            center_y,
            stride,
            dist_center,
            step,
            ..
        } = *self;
        let channels = pixel_format.channels() as usize;

        for wallpaper in wallpapers.iter() {
            wallpaper
                .borrow_mut()
                .canvas_change(backend, objman, pixel_format, |canvas| {
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
                            let new = unsafe { img.get_unchecked(line * stride + col) };
                            change_byte(step, old, new);
                        }
                    }
                });
        }

        self.dist_center = self.seq.now();
        self.seq.advance_to(self.start.elapsed().as_secs_f64());
        self.start.elapsed().as_secs_f64() > self.seq.duration()
    }
}

struct Outer {
    start: Instant,
    seq: AnimationSequence<f32>,
    width: usize,
    height: usize,
    center_x: usize,
    center_y: usize,
    stride: usize,
    dist_center: f32,
    step: u8,
}

impl Outer {
    fn new(transition: &Transition, pixel_format: PixelFormat, dimensions: (u32, u32)) -> Self {
        let (width, height) = (dimensions.0 as f32, dimensions.1 as f32);
        let (center_x, center_y) = transition.pos.to_pixel(dimensions, transition.invert_y);
        let dist_center = {
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

        let step = transition.step.get();
        let channels = pixel_format.channels() as usize;
        let stride = width * channels;
        let (seq, start) = bezier_seq(transition, dist_center, 0.0);
        Self {
            step,
            start,
            seq,
            width,
            height,
            center_x,
            center_y,
            stride,
            dist_center,
        }
    }
    fn run(
        &mut self,
        backend: &mut Waybackend,
        objman: &mut ObjectManager<WaylandObject>,
        pixel_format: PixelFormat,
        wallpapers: &mut [Rc<RefCell<Wallpaper>>],
        img: &[u8],
    ) -> bool {
        let Self {
            width,
            height,
            center_x,
            center_y,
            stride,
            dist_center,
            step,
            ..
        } = *self;
        let channels = pixel_format.channels() as usize;
        for wallpaper in wallpapers.iter() {
            wallpaper
                .borrow_mut()
                .canvas_change(backend, objman, pixel_format, |canvas| {
                    // to plot half a circle with radius r, we do sqrt(r^2 - x^2)
                    for line in 0..height {
                        let offset = (dist_center.powi(2) - (center_y as f32 - line as f32).powi(2))
                            .sqrt() as usize;
                        let col_begin = center_x.saturating_sub(offset) * channels;
                        let col_end = width.min(center_x + offset) * channels;
                        for col in 0..col_begin {
                            let old = unsafe { canvas.get_unchecked_mut(line * stride + col) };
                            let new = unsafe { img.get_unchecked(line * stride + col) };
                            change_byte(step, old, new);
                        }
                        for col in col_end..stride {
                            let old = unsafe { canvas.get_unchecked_mut(line * stride + col) };
                            let new = unsafe { img.get_unchecked(line * stride + col) };
                            change_byte(step, old, new);
                        }
                    }
                });
        }
        self.dist_center = self.seq.now();
        self.seq.advance_to(self.start.elapsed().as_secs_f64());
        self.start.elapsed().as_secs_f64() > self.seq.duration()
    }
}
