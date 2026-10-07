fn main() {
    // 固定浅色 Fluent，组件外观不会随各机器的默认主题发生变化。
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-light".into());
    if let Err(error) = slint_build::compile_with_config("ui/app.slint", config) {
        eprintln!("Slint UI 编译失败：{error}");
        std::process::exit(1);
    }
}
