//use log::{debug, error, info, warn};
use smithay_client_toolkit::{
    environment::{Environment, SimpleGlobal},
    output::{OutputHandler, XdgOutputHandler},
    reexports::{
        client::{
            protocol::{wl_compositor, wl_output, wl_shm},
            Display, EventQueue,
        },
        protocols::{
            unstable::xdg_output::v1::client::zxdg_output_manager_v1,
            wlr::unstable::layer_shell::v1::client::zwlr_layer_shell_v1,
        },
    },
    shm::ShmHandler,
};

pub struct Env {
    compositor: SimpleGlobal<wl_compositor::WlCompositor>,
    shm: ShmHandler,
    outputs: OutputHandler,
    xdg_out: XdgOutputHandler,
    layer_shell: SimpleGlobal<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
}

smithay_client_toolkit::environment!(Env,
singles = [
wl_compositor::WlCompositor => compositor,
zwlr_layer_shell_v1::ZwlrLayerShellV1 => layer_shell,
wl_shm::WlShm => shm,
zxdg_output_manager_v1::ZxdgOutputManagerV1 => xdg_out
],
multis = [
    wl_output::WlOutput => outputs,
]);

impl ::smithay_client_toolkit::output::OutputHandling for Env {
    fn listen<F>(&mut self, f: F) -> ::smithay_client_toolkit::output::OutputStatusListener
    where
        F: FnMut(
                ::smithay_client_toolkit::reexports::client::protocol::wl_output::WlOutput,
                &::smithay_client_toolkit::output::OutputInfo,
                ::smithay_client_toolkit::reexports::client::DispatchData,
            ) + 'static,
    {
        self.outputs.listen(f)
    }
}

pub fn make_wayland_environment() -> (Environment<Env>, Display, EventQueue) {
    let display = Display::connect_to_env().expect("Failed to connect to wayland environment");
    let mut event_queue = display.create_event_queue();
    let queue_token = event_queue.token();
    let attached_display = display.attach(queue_token);

    let (outputs, xdg_out) = XdgOutputHandler::new_output_handlers();
    let env = Environment::new(
        &attached_display,
        &mut event_queue,
        Env {
            compositor: SimpleGlobal::new(),
            //subcompositor:
            shm: ShmHandler::new(),
            layer_shell: SimpleGlobal::new(),
            xdg_out,
            outputs,
        },
    )
    .expect("Couldn't create wayland environment");
    (env, display, event_queue)
}
