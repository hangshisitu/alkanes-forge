use std::path::Path;

fn main() {
    // 获取 OUT_DIR
    let out_dir = std::env::var("OUT_DIR").unwrap();

    // 设置重新构建的触发条件
    println!("cargo:rerun-if-changed=src/");
    println!("cargo:rerun-if-changed=Cargo.toml");

    // 计算 wasm 文件路径
    let wasm_path = Path::new(&out_dir)
        .ancestors()
        .nth(5) // 回到 target 目录
        .unwrap()
        .join("wasm32-unknown-unknown")
        .join("release")
        .join("forge_stoken.wasm");

    // 静默检查 wasm 文件是否存在，不输出警告
    if wasm_path.exists() {
        println!("cargo:warning=WASM file ready: {:?}", wasm_path);
    }
}