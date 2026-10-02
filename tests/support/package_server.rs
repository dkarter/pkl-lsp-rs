#[path = "../../src/formatter.rs"]
#[allow(dead_code)]
mod formatter;
#[path = "../../src/projects.rs"]
#[allow(dead_code)]
mod projects;
#[path = "../../src/schema.rs"]
#[allow(dead_code)]
mod schema;
#[path = "../../src/server.rs"]
#[allow(dead_code)]
mod server;

fn download(uri: &str) -> Option<Vec<u8>> {
    schema::package_archive_with_fetch(uri, |url, limit| {
        use std::io::{Read, Write};
        let address = std::env::args().nth(1)?;
        let mut stream = std::net::TcpStream::connect(address).ok()?;
        writeln!(stream, "{url}").ok()?;
        let mut bytes = Vec::new();
        stream.take(limit + 1).read_to_end(&mut bytes).ok()?;
        (bytes.len() as u64 <= limit).then_some(bytes)
    })
}

fn main() {
    // This separate, opt-in test executable never changes the shipped transport.
    if server::run_with_fetcher(
        std::io::BufReader::new(std::io::stdin()),
        std::io::stdout(),
        download,
    )
    .is_err()
    {
        std::process::exit(1);
    }
}
