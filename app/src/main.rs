fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ui") => slat::ui::run(&args[1..]),
        _ => slat::agent::run(),
    }
}
