use std::path::PathBuf;

use waybackend_scanner::WaylandProtocol;

fn main() {
    let out_dir = std::env::var_os("OUT_DIR").expect("missing OUT_DIR environment variable");

    let mut filepath = PathBuf::from(out_dir);
    filepath.push("wayland_protocols.rs");
    let file = std::fs::File::create(filepath).expect("failed to create wayland_protocols.rs");

    waybackend_scanner::build_script_generate(
        &[
            WaylandProtocol::Client,
            WaylandProtocol::System(PathBuf::from_iter(&[
                "stable",
                "viewporter",
                "viewporter.xml",
            ])),
            WaylandProtocol::System(PathBuf::from_iter(&[
                "staging",
                "fractional-scale",
                "fractional-scale-v1.xml",
            ])),
            WaylandProtocol::Local(PathBuf::from_iter(&[
                "../",
                "protocols",
                "wlr-layer-shell-unstable-v1.xml",
            ])),
        ],
        &file,
    );
}
