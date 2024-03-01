fn main() {
    pkg_config::Config::new()
        .atleast_version("1.8")
        .probe("liblz4")
        .unwrap();
}
