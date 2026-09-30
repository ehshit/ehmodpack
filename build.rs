fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    #[cfg(target_os = "windows")]
    {
        let icon = ".github/icon/icon-windows.ico";
        println!("cargo:rerun-if-changed={icon}");
        if let Err(why) = winres::WindowsResource::new()
            .set_icon(icon)
            .set("FileDescription", "eh's modpack")
            .set("ProductName", "ehmodpack")
            .compile()
        {
            println!("cargo:warning=could not embed the windows icon: {why}");
        }
    }
}