fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let code = colibri_rust::cli::run(args);
    std::process::exit(code);
}
