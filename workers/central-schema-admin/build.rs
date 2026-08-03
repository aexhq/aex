//! Rebuild the schema-admin executable whenever its embedded bundle changes.

fn main() {
    // `sqlx::migrate!` embeds every migration in the executable. Watching the
    // directory (rather than only today's files) makes adding a new migration
    // invalidate the Cargo artifact as well.
    println!("cargo:rerun-if-changed=../../migrations/central");
}
