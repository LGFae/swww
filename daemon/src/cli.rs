use crate::wayland::zwlr_layer_shell_v1::Layer;
use common::ipc::PixelFormat;

pub struct Cli {
    pub format: Option<PixelFormat>,
    pub quiet: bool,
    pub no_cache: bool,
    pub layer: Layer,
}

impl Cli {
    pub fn new() -> Self {
        let mut quiet = false;
        let mut no_cache = false;
        let mut format = None;
        let mut layer = Layer::background;
        let mut args = std::env::args();
        args.next(); // skip the first argument

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-f" | "--format" => match args.next().as_deref() {
                    Some("xrgb") => format = Some(PixelFormat::Xrgb),
                    Some("xbgr") => format = Some(PixelFormat::Xbgr),
                    Some("rgb") => format = Some(PixelFormat::Rgb),
                    Some("bgr") => format = Some(PixelFormat::Bgr),
                    _ => {
                        eprintln!("`--format` command line option must be one of: 'xrgb', 'xbgr', 'rgb' or 'bgr'");
                        std::process::exit(-2);
                    }
                },
                "-l" | "--layer" => {
                    match args.next().as_deref() {
                        Some("background") => layer = Layer::background,
                        Some("bottom") => layer = Layer::bottom,
                        _ => {
                            eprintln!("`--layer` command line option must be one of: 'background', 'bottom'");
                            std::process::exit(-3);
                        }
                    }
                }
                "-q" | "--quiet" => quiet = true,
                "--no-cache" => no_cache = true,
                "-h" | "--help" => {
                    println!(
                        "\
swww-daemon

Options:

    -f|--format <xrgb|xbgr|rgb|bgr>
        Force the use of a specific wl_shm format.

        It is generally better to let swww-daemon chose for itself.
        Only use this as a workaround when you run into problems.
        Whatever you chose, make sure you compositor actually supports it!
        'xrgb' is the most compatible one.

    -l|--layer <background|bottom>
        Which layer to display the background in. Defaults to `background`.

        We do not accept layers `top` and `overlay` because those would make
        your desktop unusable by simply putting an image on top of everything
        else. If there is ever a use case for these, we can reconsider this.

    --no-cache
        Don't search the cache for the last wallpaper for each output.
        Useful if you always want to select which image 'swww' loads manually
        using 'swww img'.

    -q|--quiet    will only log errors
    -h|--help     print help
    -V|--version  print version"
                    );
                    std::process::exit(0);
                }
                "-V" | "--version" => {
                    println!("swww-daemon {}", env!("CARGO_PKG_VERSION"));
                    std::process::exit(0);
                }
                s => {
                    eprintln!("Unrecognized command line argument: {s}");
                    eprintln!("Run -h|--help to know what arguments are recognized!");
                    std::process::exit(-1);
                }
            }
        }

        Self {
            format,
            quiet,
            no_cache,
            layer,
        }
    }
}
