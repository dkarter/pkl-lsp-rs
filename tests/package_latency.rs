#![cfg(feature = "test-fixture")]

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

#[path = "support/protocol.rs"]
mod protocol;

const PACKAGE: &str = "package://github.com/example/fixture/releases/download/v1/schema@1";

struct Client {
    child: Child,
    input: Option<std::process::ChildStdin>,
    output: mpsc::Receiver<Value>,
    notifications: std::cell::RefCell<Vec<Value>>,
}

impl Client {
    fn new(address: std::net::SocketAddr) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_package-fixture-server"))
            .arg(address.to_string())
            .env("PATH", "/nonexistent")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, output) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(message) = protocol::receive(&mut reader) {
                if tx.send(message).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            input: Some(input),
            output,
            notifications: Default::default(),
        }
    }

    fn send(&mut self, message: Value) {
        protocol::send(self.input.as_mut().unwrap(), &message);
    }

    fn response(&self, id: i64) -> Value {
        loop {
            let message = self
                .output
                .recv_timeout(Duration::from_secs(2))
                .expect("protocol blocked by package fetch");
            if message["id"] == id {
                return message;
            }
            assert!(
                message.get("id").is_none(),
                "unexpected/duplicate response: {message}"
            );
            self.notifications.borrow_mut().push(message);
        }
    }

    fn request(&mut self, id: i64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        self.response(id)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn blocked_fetch_does_not_block_protocol(explicit: bool) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = Client::new(listener.local_addr().unwrap());
    let (started_tx, started) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let fixture = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(&mut stream).read_line(&mut request).unwrap();
        started_tx.send(()).unwrap();
        // No HTTP timing assumptions: hold metadata until the protocol assertions finish.
        let _ = released.recv();
    });
    client.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    client.response(1);
    let uri = "file:///fixture.pkl";
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":uri,"version":1,"text":format!("amends \"{PACKAGE}#/Config.pkl\"\nmin")}}}));
    client.send(if explicit {
        json!({"jsonrpc":"2.0","id":2,"method":"pkl/downloadPackage","params":PACKAGE})
    } else {
        json!({"jsonrpc":"2.0","id":2,"method":"textDocument/completion","params":{"textDocument":{"uri":uri},"position":{"line":1,"character":3}}})
    });
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(
        client.request(12, "pkl/syncProjects", Value::Null)["result"],
        Value::Null
    );
    assert!(
        client.request(
            13,
            "textDocument/formatting",
            json!({"textDocument":{"uri":uri},"options":{"insertSpaces":true,"tabSize":2}})
        )["result"]
            .is_array()
    );
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///base.pkl","version":1,"text":"module Base\nfeatureFlag: Boolean"}}}));
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///local-import.pkl","version":1,"text":format!("amends \"{PACKAGE}#/Config.pkl\"\nimport \"./base.pkl\" as Base\nvalue = Base.featureFlag")}}}));
    assert_eq!(client.request(10, "textDocument/completion", json!({"textDocument":{"uri":"file:///local-import.pkl"},"position":{"line":2,"character":16}}))["result"][0]["label"], "featureFlag");
    assert!(client.request(11, "textDocument/hover", json!({"textDocument":{"uri":"file:///local-import.pkl"},"position":{"line":2,"character":15}}))["result"]["contents"]["value"].as_str().unwrap().contains("featureFlag"));
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"changed = 1"}]}}));
    client.send(json!({"jsonrpc":"2.0","id":3,"method":"pkl/fileContents","params":{"uri":uri}}));
    assert_eq!(client.response(3)["result"], "changed = 1");
    assert!(client.notifications.borrow().iter().any(|message| {
        message["method"] == "textDocument/publishDiagnostics"
            && message["params"]["uri"] == uri
            && message["params"]["version"] == 2
    }));
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":uri}}}));
    client.send(json!({"jsonrpc":"2.0","id":4,"method":"pkl/fileContents","params":{"uri":uri}}));
    assert_eq!(client.response(4)["result"], Value::Null);
    assert!(client.notifications.borrow().iter().any(|message| {
        message["method"] == "textDocument/publishDiagnostics"
            && message["params"]["uri"] == uri
            && message["params"].get("version").is_none()
            && message["params"]["diagnostics"] == json!([])
    }));
    client.send(json!({"jsonrpc":"2.0","id":5,"method":"shutdown"}));
    client.response(5);
    client.send(json!({"jsonrpc":"2.0","method":"exit"}));
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = client.child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "exit waited for download"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(release);
    fixture.join().unwrap();
}

#[test]
fn slow_schema_fetch_does_not_block_documents_requests_or_shutdown() {
    blocked_fetch_does_not_block_protocol(false);
}

#[test]
fn unavailable_download_does_not_block_documents_requests_or_shutdown() {
    blocked_fetch_does_not_block_protocol(true);
}

fn archive() -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("Config.pkl", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"module Config\nmin_hk_version: String")
        .unwrap();
    zip.finish().unwrap().into_inner()
}

fn verified_fixture(
    bad_checksum: bool,
) -> (
    Client,
    mpsc::Sender<()>,
    mpsc::Receiver<()>,
    std::thread::JoinHandle<()>,
) {
    use sha2::{Digest, Sha256};
    let zip = archive();
    let checksum = if bad_checksum {
        "0".repeat(64)
    } else {
        format!("{:x}", Sha256::digest(&zip))
    };
    let zip_url = format!(
        "https://{}.zip",
        PACKAGE.strip_prefix("package://").unwrap()
    );
    let metadata = serde_json::to_vec(
        &json!({"packageZipUrl":zip_url,"packageZipChecksums":{"sha256":checksum}}),
    )
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let client = Client::new(listener.local_addr().unwrap());
    let (release, released) = mpsc::channel::<()>();
    let (started_tx, started) = mpsc::channel();
    let fixture = std::thread::spawn(move || {
        for (index, body) in [metadata, zip].into_iter().enumerate() {
            listener.set_nonblocking(true).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(4);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "fixture fetch never arrived"
                        );
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = String::new();
            BufReader::new(&mut stream).read_line(&mut request).unwrap();
            if index == 0 {
                started_tx.send(()).unwrap();
                if released.recv().is_err() {
                    return;
                }
            }
            stream.write_all(&body).unwrap();
        }
    });
    (client, release, started, fixture)
}

#[test]
fn verified_download_is_deduplicated_and_cached_for_completion_and_contents() {
    let (mut client, release, started, fixture) = verified_fixture(false);
    client.send(json!({"jsonrpc":"2.0","id":1,"method":"pkl/downloadPackage","params":PACKAGE}));
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    client.send(json!({"jsonrpc":"2.0","id":2,"method":"pkl/downloadPackage","params":PACKAGE}));
    // Barrier: the second request has been processed before releasing the fetch.
    client.request(3, "pkl/fileContents", json!({"uri":"file:///missing.pkl"}));
    release.send(()).unwrap();
    assert_eq!(client.response(1)["result"], Value::Null);
    assert_eq!(client.response(2)["result"], Value::Null);
    fixture.join().unwrap();
    // Fixture has closed: subsequent success requires the verified in-memory cache.
    assert!(
        client
            .request(4, "pkl/downloadPackage", json!(PACKAGE))
            .get("error")
            .is_none()
    );
    let member = format!("{PACKAGE}#/Config.pkl");
    assert!(
        client.request(5, "pkl/fileContents", json!({"uri":member}))["result"]
            .as_str()
            .unwrap()
            .contains("min_hk_version")
    );
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///hk.pkl","version":1,"text":format!("amends \"{member}\"\nmin")}}}));
    assert_eq!(
        client.request(
            6,
            "textDocument/completion",
            json!({"textDocument":{"uri":"file:///hk.pkl"},"position":{"line":1,"character":3}})
        )["result"][0]["label"],
        "min_hk_version"
    );
}

#[test]
fn checksum_failure_is_not_cached_as_verified_or_retried_immediately() {
    let (mut client, release, started, fixture) = verified_fixture(true);
    client.send(json!({"jsonrpc":"2.0","id":1,"method":"pkl/downloadPackage","params":PACKAGE}));
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    release.send(()).unwrap();
    assert_eq!(client.response(1)["error"]["code"], -32000);
    fixture.join().unwrap();
    assert_eq!(
        client.request(2, "pkl/downloadPackage", json!(PACKAGE))["error"]["code"],
        -32000
    );
    assert_eq!(
        client.request(
            3,
            "pkl/fileContents",
            json!({"uri":format!("{PACKAGE}#/Config.pkl")})
        )["result"],
        Value::Null
    );
}

#[test]
fn cancelled_completion_does_not_block_or_publish_a_late_response() {
    let (mut client, release, started, fixture) = verified_fixture(false);
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///hk.pkl","version":1,"text":format!("amends \"{PACKAGE}#/Config.pkl\"\nmin")}}}));
    client.send(json!({"jsonrpc":"2.0","id":1,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///hk.pkl"},"position":{"line":1,"character":3}}}));
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    client.send(json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":1}}));
    assert_eq!(client.response(1)["error"]["code"], -32800);
    release.send(()).unwrap();
    fixture.join().unwrap();
    assert!(
        client
            .request(2, "pkl/downloadPackage", json!(PACKAGE))
            .get("error")
            .is_none()
    );
    assert!(
        client.output.try_recv().is_err(),
        "late duplicate cancellation response"
    );
}

#[test]
fn deferred_completion_rechecks_changed_document_instead_of_old_schema() {
    let (mut client, release, started, fixture) = verified_fixture(false);
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///hk.pkl","version":1,"text":format!("amends \"{PACKAGE}#/Config.pkl\"\nmin")}}}));
    client.send(json!({"jsonrpc":"2.0","id":1,"method":"textDocument/completion","params":{"textDocument":{"uri":"file:///hk.pkl"},"position":{"line":1,"character":3}}}));
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"file:///hk.pkl","version":2},"contentChanges":[{"text":"minLocal = 1\nmin"}]}}));
    client.request(2, "pkl/fileContents", json!({"uri":"file:///hk.pkl"}));
    release.send(()).unwrap();
    assert_eq!(client.response(1)["result"][0]["label"], "minLocal");
    fixture.join().unwrap();
}

#[test]
fn stdin_eof_cancels_deferred_requests_without_waiting_for_network() {
    let (mut client, release, started, fixture) = verified_fixture(false);
    client.send(json!({"jsonrpc":"2.0","id":1,"method":"pkl/downloadPackage","params":PACKAGE}));
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(client.input.take());
    assert_eq!(client.response(1)["error"]["code"], -32800);
    // Drop wakes fixture cleanup, without requiring a second archive connection.
    drop(release);
    fixture.join().unwrap();
}

#[test]
fn pending_request_limit_returns_retryable_error_without_blocking() {
    let (mut client, release, started, fixture) = verified_fixture(false);
    client.send(json!({"jsonrpc":"2.0","id":1,"method":"pkl/downloadPackage","params":PACKAGE}));
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    for id in 2..=128 {
        client
            .send(json!({"jsonrpc":"2.0","id":id,"method":"pkl/downloadPackage","params":PACKAGE}));
    }
    assert_eq!(
        client.request(129, "pkl/downloadPackage", json!(PACKAGE))["error"]["code"],
        -32000
    );
    client.request(130, "shutdown", Value::Null);
    drop(release);
    fixture.join().unwrap();
}

#[test]
fn worker_limit_rejects_fifth_package_while_four_downloads_are_stalled() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = Client::new(listener.local_addr().unwrap());
    let (started_tx, started) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let fixture = std::thread::spawn(move || {
        let mut streams = Vec::new();
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = String::new();
            BufReader::new(&mut stream).read_line(&mut request).unwrap();
            streams.push(stream);
            started_tx.send(()).unwrap();
        }
        let _ = released.recv();
    });
    for id in 1..=4 {
        client.send(json!({"jsonrpc":"2.0","id":id,"method":"pkl/downloadPackage","params":format!("{PACKAGE}{id}")}));
        started.recv_timeout(Duration::from_secs(2)).unwrap();
    }
    assert_eq!(
        client.request(5, "pkl/downloadPackage", json!(format!("{PACKAGE}5")))["error"]["code"],
        -32000
    );
    client.request(6, "shutdown", Value::Null);
    drop(release);
    fixture.join().unwrap();
}

#[test]
fn unsafe_package_member_is_rejected_without_starting_a_download() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = Client::new(listener.local_addr().unwrap());
    client.send(json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///unsafe.pkl","version":1,"text":format!("amends \"{PACKAGE}#/../Config.pkl\"\nmin")}}}));
    assert_eq!(
        client.request(
            1,
            "textDocument/completion",
            json!({"textDocument":{"uri":"file:///unsafe.pkl"},"position":{"line":1,"character":3}})
        )["result"],
        json!([])
    );
    listener.set_nonblocking(true).unwrap();
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
