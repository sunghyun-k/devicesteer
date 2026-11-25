//! Gives `devicesteer.exe` Windows version information (ProductName DeviceSteer, the package
//! version) and the app's icon. Code signing for open-source projects (SignPath) requires every
//! signed file's product name and version to match the project's, and without this a Rust binary
//! has none. Tauri does the same for the app's exe. Nothing happens for other targets.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let icon = "../../app/src-tauri/icons/icon.ico";
    println!("cargo:rerun-if-changed={icon}");
    let mut res = tauri_winres::WindowsResource::new();
    // The defaults are the package's name (`devicesteer-cli`) and description.
    res.set("ProductName", "DeviceSteer")
        .set("FileDescription", "DeviceSteer command-line tool")
        .set("InternalName", "devicesteer")
        .set("OriginalFilename", "devicesteer.exe")
        .set_icon(icon);
    if let Err(e) = res.compile() {
        panic!("could not embed the Windows version information: {e}");
    }
}
