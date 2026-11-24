fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        println!("cargo:rerun-if-changed=assets/app.rc");
        embed_resource::compile("assets/app.rc", embed_resource::NONE)
            .manifest_required()
            .expect("Unable to embed the Windows application icon");
    }
}
