use kestrel_engine::run;

fn main() {
    if let Err(err) = pollster::block_on(run()) {
        eprintln!("Application error: {err:?}");
    }
}
