//! Command-line stub for converting Aseprite JSON exports into the engine atlas timeline format.
//! The real implementation will parse Aseprite frame/tag data and emit `atlas.json` snippets.

use std::env;
use std::path::PathBuf;

fn main() {
    let mut args = env::args().skip(1);
    let input_path = args.next().map(PathBuf::from);
    let output_path = args.next().map(PathBuf::from);

    if input_path.is_none() || output_path.is_none() {
        eprintln!("Usage: aseprite_to_atlas <input.json> <output.json>");
        eprintln!("Stub importer – no conversion performed yet.");
        std::process::exit(1);
    }

    let input = input_path.unwrap();
    let output = output_path.unwrap();

    println!(
        "[aseprite_to_atlas] Stub invoked with input: '{}' and output: '{}'",
        input.display(),
        output.display()
    );
    println!("Conversion not implemented yet – this stub simply records the planned workflow.");
}
