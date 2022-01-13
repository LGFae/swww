use assert_cmd::Command;

fn cmd() -> Command {
    return Command::cargo_bin("fswww").unwrap();
}

const TEST_IMGS: [&str; 3] = [
    "test_images/test1.jpg",
    "test_images/test1.png",
    "test_images/test2.jpg",
];

/// # HOW TO TO RUN THESE TESTS
///
/// Make sure fswww-daemon is running in a different terminal. You should track its log there to
/// see if anything weird pops up. Then, run `cargo test`.
///
/// By the time they end, they should automatically kill the daemon.
#[ignore]
fn main() {
    sending_imgs();
    sending_img_that_does_not_exist();
    sending_imgs_with_filter();
    let output = query_outputs();
    sending_img_to_individual_monitors(&output);
    sending_img_to_monitor_that_does_not_exist();
    killing_daemon();
    cmd().arg("query").assert().failure(); //daemon is dead, so this should fail
}

fn sending_imgs() {
    for img in TEST_IMGS {
        cmd().arg("img").arg(img).assert().success();
    }
}

fn sending_img_that_does_not_exist() {
    cmd().arg("img").arg("I don't exist").assert().failure();
}

fn query_outputs() -> String {
    let output = cmd().arg("query").output().expect("Query failed!");
    let stdout = String::from_utf8(output.stdout).unwrap();
    stdout.split_once(' ').unwrap().0.to_string()
}

fn sending_img_to_individual_monitors(output: &str) {
    cmd()
        .arg("img")
        .arg(TEST_IMGS[0])
        .arg("-o")
        .arg(output)
        .assert()
        .success();
}

fn sending_img_to_monitor_that_does_not_exist() {
    cmd()
        .arg("img")
        .arg(TEST_IMGS[0])
        .arg("-o")
        .arg("AHOY")
        .assert()
        .failure();
}

fn sending_imgs_with_filter() {
    for filter in ["Nearest", "Triangle", "CatmullRom", "Gaussian", "Lanczos3"] {
        cmd()
            .arg("img")
            .arg(TEST_IMGS[0])
            .arg("-f")
            .arg(filter)
            .assert()
            .success();
    }
}

fn killing_daemon() {
    cmd().arg("kill").assert().success();
}
