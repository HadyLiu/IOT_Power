fn main() {
    // 让图标改动触发重新编译
    println!("cargo:rerun-if-changed=../packaging/icons/iot-power.ico");
    println!("cargo:rerun-if-changed=../packaging/icons/png/256x256.png");

    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../packaging/icons/iot-power.ico");
    res.set("ProductName", "IOT_Power");
    res.set("FileDescription", "DC3005U power supply controller");
    res.set("LegalCopyright", "IOT_Power developers");
    if let Err(e) = res.compile() {
        // 交叉编译时若缺 windres，只警告不打断构建
        println!("cargo:warning=winresource: {e}");
    }
}
