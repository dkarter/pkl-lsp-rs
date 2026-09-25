mod server;

fn main() {
    if let Err(error) = server::run(std::io::stdin().lock(), std::io::stdout().lock()) {
        eprintln!("pkl-lsp-rs: {error}");
        std::process::exit(1);
    }
}
