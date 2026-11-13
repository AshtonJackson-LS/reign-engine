// Embeds the Reign icon in a Windows executable. Included by the build
// scripts of the hub, the editor and the installer.
fn embed_icon() {
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
