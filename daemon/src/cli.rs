use crate::wayland::zwlr_layer_shell_v1::Layer;
use common::ipc::PixelFormat;

pub struct Cli {
    pub format: Option<PixelFormat>,
    pub quiet: bool,
    pub no_cache: bool,
    pub layer: Layer,
    pub namespace: String,
}

impl Cli {
    pub fn new() -> Self {
        let mut quiet = false;
        let mut no_cache = false;
        let mut format = None;
        let mut layer = Layer::background;
        let mut namespace = String::new();
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
                "-n" | "--namespace" => {
                    namespace = match args.next() {
                        Some(s) => s,
                        None => {
                            eprintln!("expected argument for option `--namespace`");
                            std::process::exit(-4);
                        }
                    }
                }
                "--no-cache" => no_cache = true,
                "-q" | "--quiet" => quiet = true,
                "-h" | "--help" => {
                    println!(
                        "\
swww-daemon

Options:

    -f|--format <xrgb|xbgr|rgb|bgr>
        Force the use of a specific wl_shm format.

        By default, swww-daemon will use xrgb, because it is most widely
        supported. Generally speaking, formats with 3 channels will use 3/4 the
        memory of formats with 4 channels. Also, bgr formats are more efficient
        than rgb formats because we do not need to do an extra swap of the bytes
        when decoding the image (though the difference is unnoticiable).

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
            namespace,
        }
    }
}
