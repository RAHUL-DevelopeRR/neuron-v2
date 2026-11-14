//! `read_document` in a child process: the CLI binary answers the hidden
//! worker argument, and the parent maps its replies and failures.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn neuron() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_neuron"))
}

fn temp_dir(label: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "neuron-worker-{label}-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// A one-page PDF showing `text` in Helvetica.
fn pdf(text: &str) -> Vec<u8> {
    let content = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_string(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut out = String::from("%PDF-1.7\n");
    for (index, body) in objects.iter().enumerate() {
        let _ = write!(out, "{} 0 obj\n{body}\nendobj\n", index + 1);
    }
    out.push_str("trailer\n<< /Root 1 0 R >>\n%%EOF\n");
    out.into_bytes()
}

#[test]
fn the_worker_reads_a_document_and_reports_errors_as_json() {
    let dir = temp_dir("direct");
    let path = dir.join("hello.pdf");
    std::fs::write(&path, pdf("Hello from the worker")).expect("write pdf");

    let mut child = Command::new(neuron())
        .arg(tools::DOCUMENT_WORKER_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start worker");
    let request = serde_json::json!({ "path": path, "pages": "1", "max_chars": 2000 });
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(request.to_string().as_bytes())
        .expect("send request");
    let output = child.wait_with_output().expect("worker output");
    assert!(output.status.success(), "{output:?}");
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json reply");
    let markdown = reply["ok"].as_str().expect("ok reply");
    assert!(markdown.contains("Hello from the worker"), "{markdown}");

    // A bad request is an error reply, not a crash.
    let output = Command::new(neuron())
        .arg(tools::DOCUMENT_WORKER_ARG)
        .stdin(Stdio::null())
        .output()
        .expect("run worker");
    assert!(output.status.success(), "{output:?}");
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json reply");
    assert!(reply["error"]
        .as_str()
        .is_some_and(|error| error.contains("bad read_document request")));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn reads_through_the_worker_return_markdown_or_the_readers_error() {
    let dir = temp_dir("parent");
    let good = dir.join("good.pdf");
    std::fs::write(&good, pdf("Parent and child")).expect("write pdf");
    let markdown = tools::read_document_with_worker(neuron(), &good, None, None).expect("reads");
    assert!(markdown.contains("Parent and child"), "{markdown}");
    assert!(
        markdown.starts_with("[good.pdf: PDF document, 1 page"),
        "{markdown}"
    );

    // The reader's own errors pass through unchanged.
    let text = dir.join("notes.pdf");
    std::fs::write(&text, "plain text, not a PDF\n").expect("write text");
    let error =
        tools::read_document_with_worker(neuron(), &text, None, None).expect_err("not a pdf");
    assert!(error.contains("read it with read_file"), "{error}");
    assert!(
        !error.contains("could not read this document safely"),
        "{error}"
    );

    // A binary that is not a worker gives an unsafe-read error, not a panic.
    let not_a_worker = if cfg!(windows) {
        PathBuf::from(std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string()))
    } else {
        PathBuf::from("/bin/sh")
    };
    let error =
        tools::read_document_with_worker(&not_a_worker, &good, None, None).expect_err("no reply");
    assert!(
        error.starts_with("could not read this document safely: "),
        "{error}"
    );
    std::fs::remove_dir_all(dir).ok();
}
