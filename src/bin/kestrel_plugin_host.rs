use std::env;
use std::io::{self, BufRead};

fn main() -> io::Result<()> {
    let mut plugin_path = String::new();
    let mut plugin_name = String::new();
    let mut capabilities: Vec<String> = Vec::new();
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--plugin" => {
                if let Some(path) = args.next() {
                    plugin_path = path;
                }
            }
            "--name" => {
                if let Some(name) = args.next() {
                    plugin_name = name;
                }
            }
            "--cap" => {
                if let Some(cap) = args.next() {
                    capabilities.push(cap);
                }
            }
            _ => {}
        }
    }

    eprintln!(
        "[isolated-host] launched for '{}' (plugin='{plugin_path}', caps={:?})",
        if plugin_name.is_empty() { "<unknown>" } else { &plugin_name },
        capabilities
    );
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        match line {
            Ok(ref text) if text.trim().eq_ignore_ascii_case("exit") => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    eprintln!("[isolated-host] shutting down '{}'", plugin_name);
    Ok(())
}
