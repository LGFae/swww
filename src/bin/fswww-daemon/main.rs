use image::imageops::FilterType;
use log::{debug, error, info, warn};
use nix::{sys::signal, unistd::Pid};
use structopt::StructOpt;

use smithay_client_toolkit::{
    environment::Environment,
    output::{with_output_info, OutputInfo},
    reexports::{
        calloop::{
            self, channel,
            signals::{Signal, Signals},
        },
        client::protocol::{wl_output, wl_shm, wl_surface},
        client::{Attached, Main},
        protocols::wlr::unstable::layer_shell::v1::client::{
            zwlr_layer_shell_v1, zwlr_layer_surface_v1,
        },
    },
    shm::AutoMemPool,
    WaylandSource,
};

use std::{
    cell::{Cell, RefCell, RefMut},
    fs,
    io::Write,
    path::{Path, PathBuf},
    rc::Rc,
    sync::mpsc,
};

mod img_processor;
use img_processor::ProcessingResult;
mod wayland;

const TMP_DIR: &str = "/tmp/fswww";
const TMP_PID: &str = "pid";
const TMP_IN: &str = "in";
const TMP_OUT: &str = "out";

#[derive(PartialEq, Copy, Clone)]
enum RenderEvent {
    Configure { width: u32, height: u32 },
    Closed,
}

struct Background {
    output_name: String,
    surface: wl_surface::WlSurface,
    layer_surface: Main<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1>,
    next_render_event: Rc<Cell<Option<RenderEvent>>>,
    pool: AutoMemPool,
    dimensions: (u32, u32),
}

impl Background {
    fn new(
        output: &wl_output::WlOutput,
        output_name: String,
        surface: wl_surface::WlSurface,
        layer_shell: &Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
        pool: AutoMemPool,
    ) -> Option<Self> {
        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            Some(output),
            zwlr_layer_shell_v1::Layer::Background,
            "fswww".to_owned(),
        );

        layer_surface.set_anchor(zwlr_layer_surface_v1::Anchor::all());
        layer_surface.set_exclusive_zone(-1);

        let next_render_event = Rc::new(Cell::new(None::<RenderEvent>));
        let next_render_event_handle = Rc::clone(&next_render_event);
        layer_surface.quick_assign(move |layer_surface, event, _| {
            match (event, next_render_event_handle.get()) {
                (zwlr_layer_surface_v1::Event::Closed, _) => {
                    next_render_event_handle.set(Some(RenderEvent::Closed));
                }
                (
                    zwlr_layer_surface_v1::Event::Configure {
                        serial,
                        width,
                        height,
                    },
                    next,
                ) if next != Some(RenderEvent::Closed) => {
                    layer_surface.ack_configure(serial);
                    next_render_event_handle.set(Some(RenderEvent::Configure { width, height }));
                }
                (_, _) => {}
            }
        });

        // Commit so that the server will send a configure event
        surface.commit();

        Some(Self {
            surface,
            layer_surface,
            next_render_event,
            pool,
            output_name,
            dimensions: (0, 0),
        })
    }

    /// Handles any events that have occurred since the last call, redrawing if needed.
    /// Returns true if the surface should be dropped.
    fn handle_events(&mut self) -> bool {
        match self.next_render_event.take() {
            Some(RenderEvent::Closed) => true,
            Some(RenderEvent::Configure { width, height }) => {
                self.dimensions = (width, height);
                false
            }
            None => false,
        }
    }

    fn draw(&mut self, img: &[u8]) {
        let stride = 4 * self.dimensions.0 as i32;
        let width = self.dimensions.0 as i32;
        let height = self.dimensions.1 as i32;

        match self
            .pool
            .buffer(width, height, stride, wl_shm::Format::Argb8888)
        {
            Ok((canvas, buffer)) => {
                canvas.copy_from_slice(img);
                info!("Copied bytes to canvas.");

                // Attach the buffer to the surface and mark the entire surface as damaged
                self.surface.attach(Some(&buffer), 0, 0);
                self.surface
                    .damage_buffer(0, 0, width as i32, height as i32);

                // Finally, commit the surface
                self.surface.commit();
            }
            Err(e) => warn!(
                "Failed to create buffer from mempoll: {}. Image won't be drawn...",
                e
            ),
        }
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        self.layer_surface.destroy();
        self.surface.destroy();
    }
}

#[derive(Debug, StructOpt)]
#[structopt(name = "fswww-daemon")]
///The fswww daemon
///
///You should never have to interact directly with the daemon, but use fswww init to start it
///instead. fswww will automatically fork the process for you, unless you run it with the
///--no-daemon option.
///Note that, if, for some reason, you decide to run fswww-daemon manually yourself, there is no
///option to fork it; you may only pass -h or --help to see this message, or -V or --version to see
///the version you are running.
struct Daemon {}

pub fn main() {
    Daemon::from_args();

    make_logger();
    info!("Starting...");
    make_tmp_files();
    info!("Created temporary files in {}.", TMP_DIR);

    let (env, display, queue) = wayland::make_wayland_environment();

    let bgs = Rc::new(RefCell::new(Vec::new()));

    let layer_shell = env.require_global::<zwlr_layer_shell_v1::ZwlrLayerShellV1>();

    let env_handle = env.clone();
    let bgs_handle = Rc::clone(&bgs);
    let output_handler = move |output: wl_output::WlOutput, info: &OutputInfo| {
        create_backgrounds(output, info, &env_handle, &bgs_handle, &layer_shell.clone())
    };
    // Process currently existing outputs
    for output in env.get_all_outputs() {
        if let Some(info) = with_output_info(&output, Clone::clone) {
            output_handler(output, &info);
        }
    }

    // Setup a listener for changes
    // The listener will live for as long as we keep this handle alive
    let _listner_handle =
        env.listen_for_outputs(move |output, info, _| output_handler(output, info));

    {
        let mut event_loop = calloop::EventLoop::<(
            bool,
            Option<channel::Channel<(Vec<String>, Vec<u8>)>>,
            Vec<mpsc::Sender<Vec<String>>>,
        )>::try_new()
        .unwrap();

        let signals = Signals::new(&[Signal::SIGUSR1, Signal::SIGUSR2]).unwrap();
        let event_handle = event_loop.handle();
        event_handle
            .insert_source(
                signals,
                |s, _, (running, new_channel, anim_senders)| match s.signal() {
                    Signal::SIGUSR1 => {
                        let mut bgs_ref = bgs.borrow_mut();
                        if let Some((outputs, filter, img)) = decode_usr1_msg(&mut bgs_ref) {
                            let mut i = 0;
                            while i < anim_senders.len() {
                                if anim_senders[i].send(outputs.clone()).is_err() {
                                    anim_senders.remove(i);
                                } else {
                                    i += 1;
                                }
                            }
                            for result in
                                send_request_to_processor(&mut bgs_ref, outputs, filter, &img)
                            {
                                match result {
                                    ProcessingResult::Img(msg) => {
                                        //NOTE: THIS LOOP IS IN THE WRONG PLACE
                                        debug!("Received img as processing result");
                                        handle_recv_msg(&mut bgs_ref, msg);
                                    }
                                    ProcessingResult::Gif((channel, sender)) => {
                                        debug!("Received gif as processing result");
                                        *new_channel = Some(channel);
                                        anim_senders.push(sender);
                                    }
                                }
                            }
                        }
                    }
                    Signal::SIGUSR2 => *running = false,
                    _ => (),
                },
            )
            .unwrap();

        WaylandSource::new(queue)
            .quick_insert(event_handle)
            .unwrap();

        send_answer(true);

        let mut v: Vec<mpsc::Sender<Vec<String>>> = Vec::new();
        loop {
            // This is ugly, let's hope that some version of drain_filter() gets stabilized soon
            // https://github.com/rust-lang/rust/issues/43244
            {
                let mut bgs = bgs.borrow_mut();
                let mut i = 0;
                while i != bgs.len() {
                    if bgs[i].handle_events() {
                        bgs.remove(i);
                    } else {
                        i += 1;
                    }
                }
            }

            //let tokens = Vec::new();
            let mut shared_data = (true, None, v.clone());
            event_loop
                .dispatch(None, &mut shared_data)
                .expect("Error during event loop!");

            if !shared_data.0 {
                break;
            }

            if let Some(channel) = shared_data.1 {
                let event_handle = event_loop.handle();
                event_handle
                    .insert_source(channel, |evt, _, (_, new_channel, _old_channels)| {
                        *new_channel = None;
                        let mut bgs = bgs.borrow_mut();
                        match evt {
                            channel::Event::Msg(msg) => handle_recv_msg(&mut bgs, msg),
                            channel::Event::Closed => (), //event_handle.kill(evt), //TODO: remove this source from loop
                        }
                    })
                    .unwrap();
            }
            v = shared_data.2;
            if let Err(e) = display.flush() {
                error!("Couldn't flush display: {}", e);
            }
        }
    }
    info!("Finished running event loop.");
    send_answer(true);
    info!("Removing... /tmp/fswww directory");
    fs::remove_dir_all("/tmp/fswww").expect("Failed to remove /tmp/fswww directory.");
}

fn make_logger() {
    //If using a debug build, we generaly want to see all the logging
    #[cfg(debug_assertions)]
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Debug)
        .init();

    //If using a release build, we let the user decide
    #[cfg(not(debug_assertions))]
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Warn)
        .init();
}

fn create_backgrounds(
    output: wl_output::WlOutput,
    info: &OutputInfo,
    env: &Environment<wayland::Env>,
    bgs: &Rc<RefCell<Vec<Background>>>,
    layer_shell: &Attached<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
) {
    if info.obsolete {
        // an output has been removed, release it
        bgs.borrow_mut().retain(|bg| bg.output_name != info.name);
        output.release();
    } else {
        // an output has been created, construct a surface for it
        let surface = env.create_surface().detach();
        let pool = env
            .create_auto_pool()
            .expect("Failed to create a memory pool!");

        debug!("New background with output: {:?}", info);
        if let Some(bg) = Background::new(&output, info.name.clone(), surface, layer_shell, pool) {
            (*bgs.borrow_mut()).push(bg);
        }
    }
}

///Returns the log file. If anything fails, we panic, since the normal functions
///of the program depend on these files
fn make_tmp_files() {
    let dir_path = Path::new(TMP_DIR);
    if !dir_path.exists() {
        fs::create_dir(dir_path).unwrap();
    }
    let pid_path = dir_path.join(TMP_PID);
    let mut pid_file = fs::File::create(pid_path).unwrap();
    let pid = std::process::id();
    pid_file.write_all(pid.to_string().as_bytes()).unwrap();

    //These two should only be made if they don't exist already
    for file in [TMP_IN, TMP_OUT] {
        let path = dir_path.join(file);
        if !path.exists() {
            fs::File::create(path).unwrap();
        }
    }
}

///The format for the message is as follows:
///  the first line contains the pid of the process that made the request
///  the second line contains the filter to use
///  the third contains the name of the outputs to put the img in
///  the fourth contains the path to the image
fn decode_usr1_msg(
    bgs: &mut RefMut<Vec<Background>>,
) -> Option<(Vec<String>, FilterType, PathBuf)> {
    match fs::read_to_string(Path::new(TMP_DIR).join(TMP_IN)) {
        Ok(content) => {
            let mut lines = content.lines();

            let _ = lines.next();
            let filter = get_filter_from_str(lines.next().unwrap());

            let outputs = lines.next().unwrap();

            let img = lines.next().unwrap();
            let img = Path::new(img).to_path_buf();

            //First, let's eliminate outputs with names that don't exist:
            let mut real_outputs: Vec<String> = Vec::with_capacity(bgs.len());
            //An empty line means all outputs
            if outputs.is_empty() {
                for bg in bgs.iter() {
                    real_outputs.push(bg.output_name.to_owned());
                }
            } else {
                for output in outputs.split(',') {
                    for bg in bgs.iter() {
                        let output = output.to_string();
                        if output == bg.output_name && !real_outputs.contains(&output) {
                            real_outputs.push(output);
                        }
                    }
                }
            }
            debug!("Requesting img for outputs: {:?}", real_outputs);
            Some((real_outputs, filter, img))
        }
        Err(e) => {
            error!(
                "Failed to read sent msg, even though this should be impossible {}",
                e
            );
            None
        }
    }
}

fn send_request_to_processor(
    bgs: &mut RefMut<Vec<Background>>,
    mut outputs: Vec<String>,
    filter: FilterType,
    img: &Path,
) -> Vec<ProcessingResult> {
    let mut processing_results = Vec::new();
    while !outputs.is_empty() {
        let mut out_same_dim = Vec::with_capacity(outputs.len());
        out_same_dim.push(outputs.pop().unwrap());
        let dim = bgs
            .iter()
            .find(|bg| bg.output_name == out_same_dim[0])
            .unwrap()
            .dimensions;
        for bg in bgs.iter().filter(|bg| outputs.contains(&bg.output_name)) {
            out_same_dim.push(bg.output_name.clone());
        }
        outputs.retain(|o| !out_same_dim.contains(o));
        debug!(
            "Sending message to processor: {:?}",
            (&out_same_dim, dim, img.to_path_buf())
        );

        //NOTE: IF THERE ARE MULTIPLE MONITORS WITH DIFFERENT SIZE, THIS WILL CALCULATE
        //THEM IN SEQUENCE, WHICH WE DON'T REALLY WANT
        processing_results.push(img_processor::processor_loop((
            out_same_dim,
            dim,
            filter,
            img,
        )));
    }
    processing_results
}

fn get_filter_from_str(s: &str) -> FilterType {
    match s {
        "Nearest" => FilterType::Nearest,
        "Triangle" => FilterType::Triangle,
        "CatmullRom" => FilterType::CatmullRom,
        "Gaussian" => FilterType::Gaussian,
        "Lanczos3" => FilterType::Lanczos3,
        _ => unreachable!(), //This is impossible because we test it before sending
    }
}

fn handle_recv_msg(bgs: &mut RefMut<Vec<Background>>, msg: (Vec<String>, Vec<u8>)) {
    let (outputs, img) = msg;
    for bg in bgs.iter_mut() {
        if outputs.contains(&bg.output_name) {
            bg.draw(&img);
        }
    }
    send_answer(true);
}

fn send_answer(ok: bool) {
    let pid = if let Ok(in_file) = fs::read_to_string(Path::new(TMP_DIR).join(TMP_IN)) {
        let pid_str = match in_file.lines().next() {
            Some(str) => str,
            None => return, //This can happen if we called the daemon directly
        };
        let proc_file = "/proc/".to_owned() + pid_str + "/cmdline";
        let program;
        match fs::read_to_string(&proc_file) {
            Ok(p) => program = p.split('\0').next().unwrap().to_owned(),
            Err(_) => return,
        }
        if !program.ends_with("fswww") {
            error!(
                "Pid in {}/{} doesn't belong to a fswww process.",
                TMP_DIR, TMP_IN
            );
            return;
        }
        pid_str.parse().unwrap()
    } else {
        error!(
            "Failed to read {}/{} for pid of calling process.",
            TMP_DIR, TMP_IN
        );
        return;
    };

    if ok {
        if let Err(e) = signal::kill(Pid::from_raw(pid), signal::SIGUSR1) {
            warn!("Failed to send signal back indicating success: {}", e);
        }
    } else {
        if let Err(e) = signal::kill(Pid::from_raw(pid), signal::SIGUSR2) {
            error!("Failed to send signal back indicating failure: {}", e);
        }
    }
}
