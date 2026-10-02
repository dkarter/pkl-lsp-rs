mod formatter;
mod projects;
mod schema;
mod server;

fn main() {
    if let Err(error) = server::run(
        std::io::BufReader::new(std::io::stdin()),
        std::io::stdout().lock(),
    ) {
        eprintln!("pkl-lsp-rs: {error}");
        std::process::exit(1);
    }
}
