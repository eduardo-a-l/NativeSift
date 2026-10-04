use std::env;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS is always set");

    let platform_source = match target_os.as_str() {
        "linux" => "cpp/watcher_linux.cpp",
        "macos" => "cpp/watcher_macos.cpp",
        "windows" => "cpp/watcher_windows.cpp",
        other => panic!("NativeSift does not support target os `{other}`"),
    };

    cxx_build::bridge("src/bridge.rs")
        .file(platform_source)
        .include("cpp")
        .std("c++17")
        .warnings(true)
        .compile("nativesift_platform_cpp");

    if target_os == "macos" {
        println!("cargo:rustc-link-lib=framework=CoreServices");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
    }

    println!("cargo:rerun-if-changed=src/bridge.rs");
    println!("cargo:rerun-if-changed=cpp");
}
