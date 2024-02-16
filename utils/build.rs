fn main() {
    pkg_config::Config::new()
        .atleast_version("1.9")
        .probe("liblz4")
        .unwrap();
}
