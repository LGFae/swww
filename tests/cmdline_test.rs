use assert_cmd::Command;

fn cmd() -> Command {
    return Command::cargo_bin("fswww").unwrap();
}

const TEST_IMGS: [&str; 3] = [
    "test_images/test1.jpg",
    "test_images/test1.png",
    "test_images/test2.jpg",
];

fn main() {
    init_daemon();
    init_daemon_twice();
    sending_imgs();
    sending_img_that_does_not_exist();
    sending_imgs_with_filter();
    sending_img_with_filter_that_does_not_exist();
    let output = query_outputs();
    sending_img_to_individual_monitors(&output);
    sending_img_to_monitor_that_does_not_exist();
    sending_img_with_no_transition();
    killing_daemon();
    cmd().arg("query").assert().failure(); //daemon is dead, so this should fail
}

fn sending_imgs() {
    for img in TEST_IMGS {
        cmd().arg("img").arg(img).assert().success();
    }
}

fn init_daemon() {
    cmd().arg("init").assert().success();
}

///This should fail since we already have an instance running
fn init_daemon_twice() {
    cmd().arg("init").assert().failure();
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
fn sending_img_with_filter_that_does_not_exist() {
    cmd()
        .arg("img")
        .arg(TEST_IMGS[0])
        .arg("-f")
        .arg("AHOY")
        .assert()
        .failure();
}

fn sending_img_with_no_transition() {
    cmd()
        .arg("img")
        .arg(TEST_IMGS[0])
        .arg("--no-transition")
        .assert()
        .success();
}

fn killing_daemon() {
    cmd().arg("kill").assert().success();
}
