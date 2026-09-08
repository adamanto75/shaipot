fn main() {
    // Build the RandomX vendored from the shaicoin node tree. NOT a crates.io
    // randomx: those carry Monero's Argon2 salt and instruction frequencies and
    // would hash to values this chain has never seen - at full speed, silently.
    let dst = cmake::Config::new("randomx")
        .define("CMAKE_BUILD_TYPE", "Release")
        .build_target("randomx")
        .build();
    println!("cargo:rustc-link-search=native={}/build", dst.display());
    println!("cargo:rustc-link-lib=static=randomx");
    // static, not dylib: an explicit dylib request here defeats
    // -static-libstdc++ below and the release binary then needs a matching
    // libstdc++ on every user machine.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "linux" {
        println!("cargo:rustc-link-lib=static=stdc++");
    } else if target_os == "macos" {
        println!("cargo:rustc-link-lib=dylib=c++");
    }
    // RandomX is C++, so the binary otherwise needs libstdc++ at runtime and a
    // release built here will not start on a host with an older one. Static
    // linking leaves libc as the only real dependency.
    if target_os == "linux" {
        println!("cargo:rustc-link-arg=-static-libstdc++");
        println!("cargo:rustc-link-arg=-static-libgcc");
    }
    println!("cargo:rerun-if-changed=randomx/src");
}
