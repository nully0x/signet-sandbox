use std::path::Path;

// The dashboard is built by `npm --prefix ui run build` and embedded into
// the API image. Fresh checkouts have no dist; stub one so the workspace
// still compiles and `GET /` explains how to build the real UI.
fn main() {
    println!("cargo:rerun-if-changed=../../ui/dist");
    let dist = Path::new("../../ui/dist");
    if dist.join("index.html").exists() {
        return;
    }
    std::fs::create_dir_all(dist).expect("create ui/dist");
    std::fs::write(
        dist.join("index.html"),
        concat!(
            "<!doctype html><title>signet dashboard</title>",
            "<p>Dashboard assets are not built.</p>",
            "<p>Run <code>npm --prefix ui install",
            " &amp;&amp; npm --prefix ui run build</code>.</p>\n",
        ),
    )
    .expect("write ui/dist stub");
}
