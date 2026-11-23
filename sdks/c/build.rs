fn main() {
    // C consumers must not retain a reference to Cargo's private build directory.
    if std::env::var("CARGO_CFG_TARGET_VENDOR").as_deref() == Ok("apple") {
        println!("cargo::rustc-link-arg-cdylib=-Wl,-install_name,@rpath/libmynofy_c.dylib");
    }
}
