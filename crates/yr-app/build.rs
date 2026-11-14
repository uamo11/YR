fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        // Link libc++abi required by libc++_static when oboe (cpal/rodio) compiles on NDK 27.
        println!("cargo:rustc-link-lib=c++abi");
    }
}
