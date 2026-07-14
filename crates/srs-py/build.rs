fn main() {
    println!("cargo::rustc-check-cfg=cfg(petek_view_schema_v6)");
    println!("cargo::rerun-if-env-changed=PETEK_VIEW_SCHEMA_V6");
    if std::env::var("PETEK_VIEW_SCHEMA_V6").as_deref() == Ok("1") {
        println!("cargo::rustc-cfg=petek_view_schema_v6");
    }
}
