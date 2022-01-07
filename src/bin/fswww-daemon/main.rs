use image::imageops::FilterType;
use log::{debug, error, info, warn};
use nix::{sys::signal, unistd::Pid};
use simplelog::{ColorChoice, LevelFilter, TermLogger, TerminalMode};
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
        client::{Attached, Display, EventQueue, Main},
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
};

mod processor;
use processor::ProcessingResult;
mod wayland;

const TMP_DIR: &str = "/tmp/fswww";
const TMP_PID: &str = "pid";
const TMP_IN: &str = "in";
const TMP_OUT: &str = "out";

#[cfg(not(debug_assertions))]
const TMP_LOG: &str = "log";

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
    decompressor: miniz_oxide::inflate::core::DecompressorOxide,
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

        let decompressor = miniz_oxide::inflate::core::DecompressorOxide::default();
        Some(Self {
            surface,
            layer_surface,
            next_render_event,
            pool,
            output_name,
            decompressor,
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
                info!("Copied img to buffer.");

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

    ///For now, same as draw, but decompresses the sent image first
    fn animate(&mut self, frame: &[u8]) {
        let stride = 4 * self.dimensions.0 as i32;
        let width = self.dimensions.0 as i32;
        let height = self.dimensions.1 as i32;

        match self
            .pool
            .buffer(width, height, stride, wl_shm::Format::Argb8888)
        {
            Ok((canvas, buffer)) => {
                self.decompressor.init();
                miniz_oxide::inflate::core::decompress(&mut self.decompressor, frame, canvas, 0, 4);
                info!("Decompressed frame.");

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
///the version you are running. Note also there is no advantage to running the daemon this way, as
///you will fail receive the confirmation message the daemon sends revealing initialization went ok
///that fswww waits for, and you won't really see any extra information, as loging is redirected to
/// /tmp/fswww/log by default in release builds.
struct Daemon {}

pub fn main() {
    Daemon::from_args();

    make_tmp_files(); //Must make this first because the file we log to is in there
    make_logger();
    info!(
        "Made temporary files in {} and initalized logger. Starting daemon...",
        TMP_DIR
    );

    let (env, display, queue) = wayland::make_wayland_environment();

    let mut bgs = Rc::new(RefCell::new(Vec::new()));

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

    //NOTE: we can't move these into the function because it causes a segfault
    run_main_loop(&mut bgs, queue, &display);

    info!("Finished running event loop.");

    send_answer(true); //in order to send the answer, we read the in file for the pid of the caller
    info!("Removing... /tmp/fswww directory");
    fs::remove_dir_all("/tmp/fswww").expect("Failed to remove /tmp/fswww directory.");
}

fn make_logger() {
    #[cfg(debug_assertions)]
    {
        let config = simplelog::ConfigBuilder::new()
            .set_thread_level(LevelFilter::Info) //let me see where the processing is happenning
            .set_time_format_str("%H:%M:%S%.f") //let me see those nanoseconds
            .build();
        TermLogger::init(
            LevelFilter::Debug,
            config,
            TerminalMode::Stderr,
            ColorChoice::AlwaysAnsi,
        )
        .expect("Failed to initialize logger. Cancelling...");
    }

    //For the release version, we log to a file, only warnings and errors, using the default config
    #[cfg(not(debug_assertions))]
    {
        simplelog::WriteLogger::init(
            LevelFilter::Warn,
            simplelog::Config::default(),
            fs::File::create(Path::new(TMP_DIR).join(TMP_LOG)).unwrap(),
        )
        .expect("Failed to initialize logger. Cancelling...");
    }
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
///of the program depend on these files.
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
    //Also we don't make the log file here, since it only needs to be made when
    //the logger writes to it (release builds)
    for file in [TMP_IN, TMP_OUT] {
        let path = dir_path.join(file);
        if !path.exists() {
            fs::File::create(path).unwrap();
        }
    }
}

///bgs and display can't be moved into here because it causes a segfault
fn run_main_loop(bgs: &mut Rc<RefCell<Vec<Background>>>, queue: EventQueue, display: &Display) {
    let (frame_sender, frame_receiver) = calloop::channel::channel();
    let mut processor = processor::Processor::new(frame_sender);

    let mut event_loop = calloop::EventLoop::<calloop::LoopSignal>::try_new().unwrap();

    let signals = Signals::new(&[Signal::SIGUSR1, Signal::SIGUSR2]).unwrap();
    let event_handle = event_loop.handle();
    event_handle
        .insert_source(signals, |s, _, loop_signal| match s.signal() {
            Signal::SIGUSR1 => {
                let mut bgs_ref = bgs.borrow_mut();
                if let Some((outputs, filter, img)) = decode_usr1_msg(&mut bgs_ref) {
                    for result in send_request_to_processor(
                        &mut bgs_ref,
                        outputs,
                        filter,
                        &img,
                        &mut processor,
                    ) {
                        debug!("Received img as processing result");
                        handle_recv_msg(&mut bgs_ref, &result);
                    }
                    send_answer(true);
                }
            }
            Signal::SIGUSR2 => loop_signal.stop(),
            _ => (),
        })
        .unwrap();

    event_handle
        .insert_source(frame_receiver, |evt, _, loop_signal| match evt {
            channel::Event::Msg(msg) => handle_recv_msg(&mut bgs.borrow_mut(), &msg),
            channel::Event::Closed => loop_signal.stop(),
        })
        .unwrap();

    WaylandSource::new(queue)
        .quick_insert(event_handle)
        .unwrap();

    let mut loop_signal = event_loop.get_signal();
    send_answer(true);
    event_loop
        .run(None, &mut loop_signal, |_| {
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
            if let Err(e) = display.flush() {
                error!("Couldn't flush display: {}", e);
            }
        })
        .expect("Event loop closed unexpectedly.");
    drop(event_loop);
}

///The format for the message is as follows:
///
///The first line contains the pid of the process that made the request
///The second line contains the filter to use
///The third contains the name of the outputs to put the img in
///The fourth contains the path to the image
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
    processor: &mut processor::Processor,
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
        processing_results.push(processor.process((out_same_dim, dim, filter, img)));
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

fn handle_recv_msg(bgs: &mut RefMut<Vec<Background>>, msg: &(Vec<String>, Vec<u8>)) {
    let (outputs, img) = msg;
    for bg in bgs.iter_mut() {
        if outputs.contains(&bg.output_name) {
            bg.draw(&img);
        }
    }
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
