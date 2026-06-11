const COMMANDS: &[&str] = &["status", "start", "stop", "restart", "logs"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).build();
}
