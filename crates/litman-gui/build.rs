fn main() {
    println!("cargo:rerun-if-changed=../../packaging/icons/litman.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winres::WindowsResource::new();
        resource
            .set_icon("../../packaging/icons/litman.ico")
            .set("CompanyName", "Jingdong Zhang")
            .set("ProductName", "LitMan")
            .set("FileDescription", "LitMan literature manager")
            .set("LegalCopyright", "Copyright © 2026 Jingdong Zhang");
        resource
            .compile()
            .expect("failed to embed the LitMan Windows resources");
    }
}
