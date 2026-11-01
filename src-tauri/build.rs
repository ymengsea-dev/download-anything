fn main() {
    // Generate the app context (config, icons, frontend assets) here rather than with
    // `generate_context!` in lib.rs, so Cargo's rerun-if-changed tracking re-embeds icons
    // and config whenever they change, including the macOS Dock icon used by `tauri dev`.
    let attributes = tauri_build::Attributes::new().codegen(tauri_build::CodegenContext::new());
    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}
