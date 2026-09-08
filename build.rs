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
    println!("cargo:rustc-link-lib=dylib=stdc++");
    println!("cargo:rerun-if-changed=randomx/src");
}
