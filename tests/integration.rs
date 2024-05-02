//! These are relatively simple tests just to make sure no basic functionally
//! was broken by anything. They are no substitute to actually trying to run
//! the program yourself and seeing if anything broke (e.g. maybe images stopped
//! rendering correctly, somehow, without the program breaking down)

use assert_cmd::Command;
use std::path::PathBuf;

const TEST_IMG_DIR: &str = "test_images";
const TEST_IMGS: [&str; 3] = [
    "test_images/test1.jpg",
    "test_images/test2.png",
    "test_images/test3.bmp",
];

fn make_img_dir() {
    let p = PathBuf::from(TEST_IMG_DIR);
    if !p.is_dir() {
        std::fs::create_dir(p)
            .expect("Failed to create directory to put the images used for testing: ");
    }
}

fn make_test_imgs() {
    make_img_dir();
    for (i, test_img) in TEST_IMGS.iter().enumerate() {
        let p = PathBuf::from(test_img);
        if !p.is_file() {
            //We use i to create images of different dimensions, just to be more through
            let mut imgbuf = image::ImageBuffer::new(400 * (i as u32 + 1), 400 * (i as u32 + 1));

            //This is taken straight from the image crate fractal example
            for (x, y, pixel) in imgbuf.enumerate_pixels_mut() {
                let r = (0.3 * x as f32) as u8;
                let b = (0.3 * y as f32) as u8;
                *pixel = image::Rgb([r, 0, b]);
            }

            imgbuf
                .save(test_img)
                .expect("Failed to create image for testing: ");
        }
    }
}

fn cmd() -> Command {
    Command::cargo_bin("swww").unwrap()
}

fn start_daemon() -> Command {
    let mut cmd = Command::cargo_bin("swww-daemon").unwrap();
    cmd.arg("--no-cache");
    cmd
}

#[test]
#[ignore]
fn general_commands() {
    make_test_imgs();

    init_daemon();
    init_daemon_twice();
    sending_imgs();
    sending_img_that_does_not_exist();
    sending_imgs_with_filter();
    sending_img_with_filter_that_does_not_exist();
    sending_img_from_stdin();
    let output = query_outputs();
    sending_img_to_individual_monitors(&output);
    sending_img_to_monitor_that_does_not_exist();
    sending_img_with_custom_transition();
    clear_outputs();
    killing_daemon();
    cmd().arg("query").assert().failure(); //daemon is dead, so this should fail
}

fn sending_imgs() {
    for img in TEST_IMGS {
        cmd().arg("img").arg(img).assert().success();
    }
}

fn init_daemon() {
    std::thread::spawn(|| {
        start_daemon().assert().success();
    });
    // sleep for a bit to allow the daemon to init correctly
    // note that even though this is a race-condition, in the actual program we
    // have implemented some proper syncronization. And, in here, it is *very*
    // unlikely that this will ever be a problem, (and, if it is, it is not a
    // very big deal, it will merely cause init_daemon_twice to false-fail)
    std::thread::sleep(std::time::Duration::from_millis(1));
}

/// Should fail since we already have an instance running
fn init_daemon_twice() {
    start_daemon().assert().failure();
}

fn sending_img_that_does_not_exist() {
    cmd().arg("img").arg("I don't exist").assert().failure();
}

fn query_outputs() -> String {
    let output = cmd().arg("query").output().expect("Query failed!");
    let stdout = String::from_utf8(output.stdout).unwrap();
    stdout.split_once(':').unwrap().0.to_string()
}

fn sending_img_to_individual_monitors(output: &str) {
    cmd()
        .arg("img")
        .arg("-t")
        .arg("none")
        .arg(TEST_IMGS[0])
        .arg("-o")
        .arg(output)
        .assert()
        .success();
}

fn sending_img_to_monitor_that_does_not_exist() {
    cmd()
        .arg("img")
        .arg("-t")
        .arg("none")
        .arg(TEST_IMGS[0])
        .arg("-o")
        .arg("AHOY")
        .assert()
        .failure();
}

fn sending_imgs_with_filter() {
    for filter in ["Nearest", "Bilinear", "CatmullRom", "Mitchell", "Lanczos3"] {
        cmd()
            .arg("img")
            .arg("-t")
            .arg("none")
            .arg(TEST_IMGS[0])
            .arg("-f")
            .arg(filter)
            .assert()
            .success();
    }
}

fn sending_img_from_stdin() {
    cmd()
        .arg("img")
        .arg("-t")
        .arg("none")
        .arg("-")
        .pipe_stdin("test_images/test1.jpg")
        .expect("failed to pipe stdin")
        .assert()
        .success();
}

fn sending_img_with_filter_that_does_not_exist() {
    cmd()
        .arg("img")
        .arg("-t")
        .arg("none")
        .arg(TEST_IMGS[0])
        .arg("-f")
        .arg("AHOY")
        .assert()
        .failure();
}

fn sending_img_with_custom_transition() {
    cmd()
        .arg("img")
        .arg("-t")
        .arg("none")
        .arg(TEST_IMGS[0])
        .arg("--transition-step")
        .arg("200")
        .assert()
        .success();
}

fn clear_outputs() {
    cmd().arg("clear").assert().success();
}

fn killing_daemon() {
    cmd().arg("kill").assert().success();
}
