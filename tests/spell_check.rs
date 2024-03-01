use std::process::Command;

/// We ignore because people might not have codespell installed, and I don't want to force anyone to
/// install codespell to e.g. run tests before installing swww. This may change in the future
#[test]
#[ignore]
fn spell_check_code_and_man_pages() {
    // Make sure no docs were generated
    let _ = std::fs::remove_dir_all("doc/generated");
    match Command::new("codespell")
        .args([
            "--enable-colors",
            "--ignore-words-list",
            "crate,statics",
            "--skip",
            "doc/generated",   // skip the generated documentation
            "src",             // client
            "daemon/src",      // daemon
            "utils/src",       // common code
            "doc",             // man pages
            "example_scripts", // scripts
            "CHANGELOG.md",
            "README.md",
        ])
        .output()
    {
        Ok(output) => {
            if !output.status.success() {
                panic!(
                    "\nstdout:{}\nstderr:{}\n",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
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
