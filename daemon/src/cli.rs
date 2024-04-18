use utils::ipc::PixelFormat;

pub struct Cli {
    pub format: Option<PixelFormat>,
    pub quiet: bool,
    pub no_cache: bool,
}

impl Cli {
    pub fn new() -> Self {
        let mut quiet = false;
        let mut no_cache = false;
        let mut format = None;
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
                "-q" | "--quiet" => quiet = true,
                "--no-cache" => no_cache = true,
                "-h" | "--help" => {
                    println!("swww-daemon");
                    println!();
                    println!("Options:");
                    println!();
                    println!("  -f|--format <xrgb|xbgr|rgb|bgr>");
                    println!("          force the use of a specific wl_shm format.");
                    println!();
                    println!(
                        "          It is generally better to let swww-daemon chose for itself."
                    );
                    println!("          Only use this as a workaround when you run into problems.");
                    println!("          Whatever you chose, make sure you compositor actually supports it!");
                    println!("          'xrgb' is the most compatible one.");
                    println!();
                    println!("  --no-cache");
                    println!(
                        "         Don't search the cache for the last wallpaper for each output."
                    );
                    println!("          Useful if you always want to select which image 'swww' loads manually using 'swww img'");
                    println!();
                    println!("  -q|--quiet    will only log errors");
                    println!("  -h|--help     print help");
                    println!("  -V|--version  print version");
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
        }
    }
}
