use log::{debug, error, info, warn};
use nix::{sys::signal, unistd::Pid};

use smithay_client_toolkit::{
    default_environment,
    environment::{Environment, SimpleGlobal},
    new_default_environment,
    output::{with_output_info, OutputInfo},
    reexports::{
        calloop::{
            self,
            channel::{self, Sender},
            signals::{Signal, Signals},
            LoopSignal,
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
};

mod img_processor;

const TMP_DIR: &str = "/tmp/fswww";
const TMP_PID: &str = "pid";
const TMP_IN: &str = "in";
const TMP_OUT: &str = "out";

default_environment!(Env,
    fields = [
        layer_shell: SimpleGlobal<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    ],
    singles = [
        zwlr_layer_shell_v1::ZwlrLayerShellV1 => layer_shell
    ],
);

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

pub fn main(origin_pid: Option<i32>) {
    make_logger();
    info!("Starting...");
    make_tmp_files();
    info!("Created temporary files in {}.", TMP_DIR);

    let (env, display, queue) =
        new_default_environment!(Env, fields = [layer_shell: SimpleGlobal::new(),])
            .expect("Initial roundtrip failed!");

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

    let thread_handle;
    {
        let (sender, receiver) = channel::channel();
        let mut event_loop = calloop::EventLoop::<LoopSignal>::try_new().unwrap();
        let (call_send, call_recv) = channel::channel();

        let signals = Signals::new(&[Signal::SIGUSR1, Signal::SIGUSR2]).unwrap();
        let event_handle = event_loop.handle();
        event_handle
            .insert_source(signals, |s, _, shared_data| match s.signal() {
                Signal::SIGUSR1 => handle_usr1(bgs.borrow_mut(), sender.clone()),
                Signal::SIGUSR2 => shared_data.stop(),
                _ => (),
            })
            .unwrap();

        event_handle
            .insert_source(call_recv, |event, _, shared_data| match event {
                calloop::channel::Event::Msg(msg) => handle_recv_msg(bgs.borrow_mut(), msg),
                calloop::channel::Event::Closed => shared_data.stop(),
            })
            .unwrap();

        thread_handle =
            std::thread::spawn(move || img_processor::handle_usr_signals(call_send, receiver));

        WaylandSource::new(queue)
            .quick_insert(event_handle)
            .unwrap();

        if origin_pid.is_some() {
            send_answer(true, origin_pid);
        }

        let mut shared_data = event_loop.get_signal();
        event_loop
            .run(None, &mut shared_data, |_shared_data| {
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
                if let Err(e) = display.flush() {
                    error!("Couldn't flush display: {}", e);
                }
            })
            .expect("Error during event loop!");
    }

    info!("Finished running event loop.");

    let pid: Option<i32> = if let Ok(in_file) = fs::read_to_string(Path::new(TMP_DIR).join(TMP_IN))
    {
        match in_file.lines().next().unwrap().parse() {
            Ok(i) => Some(i),
            Err(_) => None,
        }
    } else {
        error!(
            "Failed to read {}/{} for pid of calling process.",
            TMP_DIR, TMP_IN
        );
        None
    };
    fs::remove_dir_all("/tmp/fswww").expect("Failed to remove /tmp/fswww directory.");
    info!("Removed /tmp/fswww directory");
    if thread_handle.join().is_ok() {
        send_answer(true, pid);
    } else {
        send_answer(false, pid);
    }
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
    env: &Environment<Env>,
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

    let in_path = dir_path.join(TMP_IN);
    fs::File::create(in_path).unwrap();
    let out_path = dir_path.join(TMP_OUT);
    fs::File::create(out_path).unwrap();
}

fn handle_usr1(bgs: RefMut<Vec<Background>>, sender: Sender<(Vec<String>, (u32, u32), PathBuf)>) {
    //The format for the string is as follows:
    //  the first line contains the pid of the process that made the request
    //  the second contains the name of the outputs to put the img in
    //  the third contains the path to the image
    match fs::read_to_string(Path::new(TMP_DIR).join(TMP_IN)) {
        Ok(content) => {
            let mut lines = content.lines();

            let _ = lines.next();

            let outputs = lines.next().unwrap();

            let img = lines.next().unwrap();
            let img = Path::new(img);

            //First, let's eliminate outputs with names that don't exist:
            let mut real_outputs: Vec<String> = Vec::with_capacity(bgs.len());
            //An empty line means all outputs
            if outputs.is_empty() {
                for bg in bgs.iter() {
                    real_outputs.push(bg.output_name.to_owned());
                }
            } else {
                for output in outputs.split(' ') {
                    for bg in bgs.iter() {
                        let output = output.to_string();
                        if output == bg.output_name && !real_outputs.contains(&output) {
                            real_outputs.push(output);
                        }
                    }
                }
            }
            //Then, we gather all those that have the same dimensions, sending the message
            //until we've sent for every output
            while !real_outputs.is_empty() {
                let mut out_same_dim = Vec::with_capacity(real_outputs.len());
                out_same_dim.push(real_outputs.pop().unwrap());
                let dim = bgs
                    .iter()
                    .find(|bg| bg.output_name == out_same_dim[0])
                    .unwrap()
                    .dimensions;
                for bg in bgs.iter().filter(|bg| bg.dimensions == dim) {
                    out_same_dim.push(bg.output_name.clone());
                    real_outputs.retain(|o| *o != bg.output_name);
                }
                debug!(
                    "Sending message to processor: {:?}",
                    (&out_same_dim, dim, img.to_path_buf())
                );
                sender.send((out_same_dim, dim, img.to_path_buf()));
            }
        }
        Err(e) => warn!("Error reading {}/{} file: {}", TMP_DIR, TMP_IN, e),
    }
}

fn handle_recv_msg(mut bgs: RefMut<Vec<Background>>, msg: Option<(Vec<String>, Vec<u8>)>) {
    debug!("Daemon received message back from processor.");
    if let Some((outputs, img)) = msg {
        for bg in bgs.iter_mut() {
            if outputs.contains(&bg.output_name) {
                bg.draw(&img);
            }
        }
        send_answer(true, None);
    }
}

fn send_answer(ok: bool, pid: Option<i32>) {
    let pid = match pid {
        Some(p) => p,
        None => {
            if let Ok(in_file) = fs::read_to_string(Path::new(TMP_DIR).join(TMP_IN)) {
                in_file.lines().next().unwrap().parse().unwrap()
            } else {
                error!(
                    "Failed to read {}/{} for pid of calling process.",
                    TMP_DIR, TMP_IN
                );
                return;
            }
        }
    };
    if ok {
        if let Err(e) = signal::kill(Pid::from_raw(pid), signal::SIGUSR1) {
            error!("Failed to send signal back indicating success: {}", e);
        }
    } else {
        if let Err(e) = signal::kill(Pid::from_raw(pid), signal::SIGUSR2) {
            error!("Failed to send signal back indicating failure: {}", e);
        }
    }
}
