fn main() {
    println!("cargo:rerun-if-changed=assets/xd.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/xd.ico")
            .compile()
            .expect("embed the xd Windows application icon");
    }
}
