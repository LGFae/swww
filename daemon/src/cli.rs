use clap::{Parser, ValueEnum};

#[derive(Clone, ValueEnum, Debug)]
pub enum LayerWrapper {
    Background,
    Bottom,
}

#[derive(Clone, ValueEnum, Debug, Copy)]
pub enum PixelFormatWrapper {
    Xrgb,
    Xbgr,
    Rgb,
    Bgr,
}

#[derive(Parser)]
#[command(version, name = "swww-daemon")]
pub struct Cli {
    /// Force the use of a specific `wl_shm` format.
    ///
    /// It is generally better to let swww-daemon chose for itself, only use this as a workaround when you run into problems.
    /// Whatever you chose, make sure you compositor actually supports it!
    /// 'xrgb' is the most compatible one.
    #[arg(short, long, default_value = None, value_enum, verbatim_doc_comment)]
    pub format: Option<PixelFormatWrapper>,

    /// Will only log errors.
    #[arg(short, long)]
    pub quiet: bool,

    /// Don't search the cache for the last wallpaper for each output.
    /// Useful if you always want to select which image 'swww' loads manually
    /// using 'swww img'.
    #[arg(long)]
    pub no_cache: bool,

    /// Which layer to display the background in.
    ///
    /// We do not accept layers `top` and `overlay` because those would make
    /// your desktop unusable by simply putting an image on top of everything
    /// else. If there is ever a use case for these, we can reconsider this.
    #[arg(short, long, default_value_t = LayerWrapper::Background, value_enum, verbatim_doc_comment)]
    pub layer: LayerWrapper,

    /// The namespace under which the daemon runs.
    #[arg(short, long, default_value_t = String::from("swww-daemon"))]
    pub namespace: String,
}
