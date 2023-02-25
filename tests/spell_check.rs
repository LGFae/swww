use std::process::Command;

// We ignore because people might not have codespell installed, and I don't want to force anyone to
// install to e.g. run tests before installing. This may change in the future
// IMPORTANT: THIS TEST WILL DELETE THE GENERATED DOC FILES (just run ./doc/gen.sh to generate them
// again)
#[test]
#[ignore]
fn spell_check_code_and_man_pages() {
    // Make sure no docs were generated
    let _ = std::fs::remove_dir_all("doc/generated");
    match Command::new("codespell")
        .args([
            "--enable-colors",
            "--ignore-words-list",
            "crate",
            "src",        // client
            "daemon/src", // daemon
            "utils/src",  // common code
            "doc",        // man pages
            "CHANGELOG.md",
            "README.md",
        ])
        .output()
    {
        Ok(output) => {
            if !output.status.success() {
                panic!("\n{}", String::from_utf8_lossy(&output.stdout));
            }
        }
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => {
                eprintln!(
                    "'codespell' not found. Please install in order to do spell checking:
                          `pip install codespell`"
                );
            }
            _ => eprintln!("{e}"),
        },
    }
}
