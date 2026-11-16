fn main() {
    // grove-projfs imports `ProjectedFSLib.dll`, absent until `Client-ProjFS` is
    // enabled; a load-time import kills startup with STATUS_DLL_NOT_FOUND. Link
    // args from grove-projfs/build.rs do not propagate to this exe.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg=/DELAYLOAD:ProjectedFSLib.dll");
        println!("cargo:rustc-link-arg=delayimp.lib");
    }
}
