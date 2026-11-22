fn main() {
    #[cfg(unix)]
    if let Err(error) = colibri_rust::cardputer::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
    #[cfg(not(unix))]
    {
        eprintln!("The Cardputer worker requires a Unix host.");
        std::process::exit(1);
    }
}
