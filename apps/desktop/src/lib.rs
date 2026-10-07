//! Slint 是应用层的适配器：读表单、提交命令、投影快照。
#![deny(unsafe_code)]
// 仅生成代码需要 Slint 的 vtable 宏；手写适配器继续禁止 unsafe。
#[allow(unsafe_code)]
mod ui {
    slint::include_modules!();
}
pub use ui::*;
mod app;
pub mod file_dialog;
mod plot_renderer;
mod presentation;
mod raster;
mod render_size;
pub use app::{DesktopApp, command_from_form};
pub use presentation::apply_snapshot;
