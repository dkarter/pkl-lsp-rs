use serde_json::Value;
use std::io::{BufRead, Write};

pub fn send(output: &mut impl Write, message: &Value) {
    let bytes = serde_json::to_vec(message).unwrap();
    write!(output, "Content-Length: {}\r\n\r\n", bytes.len()).unwrap();
    output.write_all(&bytes).unwrap();
    output.flush().unwrap();
}

pub fn receive(input: &mut impl BufRead) -> Option<Value> {
    let mut header = String::new();
    if input.read_line(&mut header).unwrap() == 0 {
        return None;
    }
    let size = header
        .trim()
        .strip_prefix("Content-Length: ")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    header.clear();
    input.read_line(&mut header).unwrap();
    assert_eq!(header, "\r\n");
    let mut body = vec![0; size];
    input.read_exact(&mut body).unwrap();
    Some(serde_json::from_slice(&body).unwrap())
}
