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
    initialization();
    sending_imgs();
    sending_img_to_individual_monitors();
    killing_daemon();
}

fn initialization() {
    cmd().arg("init").assert().success();
}

fn sending_imgs() {
    for img in TEST_IMGS {
        cmd().arg("img").arg(img).assert().success();
    }
}

fn sending_img_to_individual_monitors() {
    //For this, we need to implement the query functionallity
}

fn killing_daemon() {
    cmd().arg("kill").assert().success();
}
