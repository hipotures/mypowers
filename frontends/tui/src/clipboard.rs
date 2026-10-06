//! Clipboard delivery: local Wayland when available, terminal OSC 52 over SSH.
use crate::network::{ClipboardTarget, Event};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    io::{self, Write},
    time::Duration,
};
use tokio::sync::mpsc;

pub async fn copy(text: String, target: ClipboardTarget, events: mpsc::Sender<Event>) {
    let ssh = ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"]
        .iter()
        .any(|name| std::env::var_os(name).is_some());
    let event = if !ssh && copy_wayland(&text).await {
        Event::Copied(true, target)
    } else {
        // The UI thread emits terminal sequences between frames, never from a worker.
        Event::ClipboardTerminal(text, target)
    };
    let _ = events.send(event).await;
}

async fn copy_wayland(text: &str) -> bool {
    use tokio::io::AsyncWriteExt;
    let copy = async {
        let mut child = tokio::process::Command::new("wl-copy")
            .args(["--type", "text/plain;charset=utf-8"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(text.as_bytes()).await?;
        drop(stdin);
        child.wait().await
    };
    matches!(tokio::time::timeout(Duration::from_secs(2), copy).await, Ok(Ok(status)) if status.success())
}

pub fn write_terminal(writer: &mut impl Write, text: &str) -> io::Result<()> {
    // Base64 keeps newlines, Unicode and embedded escape sequences out of terminal control data.
    let sequence = format!("\x1b]52;c;{}\x07", STANDARD.encode(text.as_bytes()));
    writer.write_all(sequence.as_bytes())?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_copy_preserves_unicode_multiline_logs_and_embedded_controls() {
        for text in [
            "",
            "hello",
            "Zażółć gęślą jaźń\nINFO: 🔋 99%\n\x1b]52;c;bad\x07",
        ] {
            let mut output = Vec::new();
            write_terminal(&mut output, text).unwrap();
            assert!(output.starts_with(b"\x1b]52;c;") && output.ends_with(b"\x07"));
            let payload = &output[7..output.len() - 1];
            assert_eq!(STANDARD.decode(payload).unwrap(), text.as_bytes());
            assert!(
                payload
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(byte))
            );
        }
    }

    #[test]
    fn terminal_copy_propagates_write_and_flush_failures() {
        struct Broken(bool);
        impl Write for Broken {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.0 {
                    Err(io::ErrorKind::BrokenPipe.into())
                } else {
                    Ok(bytes.len())
                }
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
        }
        for fail_write in [true, false] {
            assert!(write_terminal(&mut Broken(fail_write), "test").is_err());
        }
    }
}
