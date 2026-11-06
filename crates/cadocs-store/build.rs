//! `sqlx::migrate!` embeds the folder at compile time, but cargo does not watch it by itself.

fn main() {
    println!("cargo:rerun-if-changed=migrations");
}
