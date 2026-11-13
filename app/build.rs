//! Embeds the Reign icon in the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=../assets/icon/reign.ico");
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../assets/icon/reign.ico");
        if let Err(e) = resource.compile() {
            // A missing Windows SDK shouldn't stop the build, only the icon.
            println!("cargo:warning=couldn't embed the icon: {e}");
        }
    }
}
