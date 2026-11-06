//! `rust-embed` needs its folder to exist, and `web/dist` is a build output that is not committed.

use std::fs;
use std::path::Path;

fn main() {
    let dist = Path::new("../../web/dist");
    fs::create_dir_all(dist).expect("cannot create web/dist");
    if !dist.join("index.html").exists() {
        println!(
            "cargo:warning=web/dist is empty, so the binary has no front end. Run `npm --prefix web ci && npm --prefix web run build` first."
        );
    }
    println!("cargo:rerun-if-changed=../../web/dist");
}
