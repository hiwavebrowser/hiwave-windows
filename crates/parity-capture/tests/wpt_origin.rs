use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::mpsc;

#[test]
fn wpt_refuses_foreign_origin_before_replay() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let (sent, requests) = mpsc::channel();
    let url = format!("http://{}/page", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(&stream);
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            let path = request.split_whitespace().nth(1).unwrap_or("").to_owned();
            sent.send(path.clone()).unwrap();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() { break; }
            }
            let body = if path == "/page" {
                "<script src='/local.js'></script><script src='http://example.invalid/foreign.js'></script><script>function add_completion_callback(){};globalThis.__hiwaveWptResult='ok'</script>"
            } else { "" };
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
    });
    let result = Command::new(env!("CARGO_BIN_EXE_parity-capture"))
        .args(["--wpt", "--url", &url]).output().unwrap();
    assert!(result.status.success(), "{:?}", result);
    let paths: Vec<_> = requests.try_iter().collect();
    assert!(paths.iter().any(|p| p == "/page"));
    assert!(paths.iter().any(|p| p == "/local.js"));
    assert!(!paths.iter().any(|p| p == "/foreign.js"));
}
