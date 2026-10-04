#![cfg(target_os = "linux")]

use std::{
    io::{self, Read, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[path = "../src/terminal.rs"]
mod terminal;

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn run_pty(command: &str, input: &[u8], resize: bool) -> String {
    let resize_command = if resize {
        "(sleep 0.6; stty rows 10 cols 40 </dev/tty; sleep 0.4; stty rows 24 cols 80 </dev/tty) & resizer=$!;"
    } else {
        ""
    };
    let shell = format!(
        "stty rows 24 cols 80; before=$(stty -g); {resize_command} {command}; result=$?; {} after=$(stty -g); if [ \"$before\" = \"$after\" ]; then printf '\\nTERMINAL_RESTORED status=%s\\n' \"$result\"; else printf '\\nTERMINAL_BROKEN\\n'; fi",
        if resize { "wait \"$resizer\";" } else { "" }
    );
    let mut child = Command::new("script")
        .args(["-q", "-e", "-f", "-c", &shell, "/dev/null"])
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env_remove("NO_COLOR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Linux PTY tests require util-linux script and stty");
    let mut stdout = child.stdout.take().unwrap();
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).unwrap();
        String::from_utf8_lossy(&output).into_owned()
    });
    if !input.is_empty() {
        // Leave time for initial rendering, telemetry, resize, and mouse reporting.
        thread::sleep(Duration::from_millis(1400));
        let (exit, interaction) = input.split_last().unwrap();
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(interaction)
            .unwrap();
        if !interaction.is_empty() {
            thread::sleep(Duration::from_millis(250));
        }
        child.stdin.as_mut().unwrap().write_all(&[*exit]).unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("PTY prototype did not exit before the deadline");
        }
        thread::sleep(Duration::from_millis(25));
    }
    let output = reader.join().unwrap();
    assert!(output.contains("TERMINAL_RESTORED"), "{output}");
    for expected in [
        "\x1b[?1049h",
        "\x1b[?1006h",
        "\x1b[?1006l",
        "\x1b[?1049l",
        "\x1b[?25h",
    ] {
        assert!(
            output.contains(expected),
            "Missing terminal sequence {expected:?}"
        );
    }
    output
}

#[test]
fn actual_binary_handles_keyboard_mouse_resize_and_quit() {
    let binary = quote(env!("CARGO_BIN_EXE_mypowers-ratatui"));
    let output = run_pty(
        &binary,
        b"adl\x1b[<35;40;16M\x1b[<0;40;16M\x1b[<0;40;16mq",
        true,
    );
    assert!(output.contains("MYPOWERS") && output.contains("AP S300 V2.0"));
    assert!(output.contains("Terminal too small"));
    assert!(output.contains("Need at least 60x18"));
    assert!(
        output.contains("38;2;"),
        "Actual terminal output must include RGB colors"
    );
    assert!(output.contains("⠋") && output.contains("⠙"));
    assert!(output.contains("TERMINAL_RESTORED status=0"));
}

#[test]
fn escape_and_control_c_restore_the_terminal() {
    let binary = quote(env!("CARGO_BIN_EXE_mypowers-ratatui"));
    for input in [b"\x1b".as_slice(), b"\x03".as_slice()] {
        let output = run_pty(&binary, input, false);
        assert!(output.contains("TERMINAL_RESTORED status=0"));
    }
}

#[test]
fn offline_and_uniform_sparklines_run_in_a_real_terminal() {
    let command = format!(
        "{} --offline --uniform-sparklines",
        quote(env!("CARGO_BIN_EXE_mypowers-ratatui"))
    );
    let output = run_pty(&command, b"q", false);
    assert!(output.contains("OFFLINE") && output.contains('●'));
    assert!(!output.contains('⠙'));
    assert!(output.contains("TERMINAL_RESTORED status=0"));
}

#[test]
fn error_and_panic_paths_restore_the_terminal() {
    for mode in ["error", "panic"] {
        let command = format!(
            "MYPOWERS_CLEANUP_PROBE={} {} --exact session_probe --nocapture",
            quote(mode),
            quote(std::env::current_exe().unwrap().to_str().unwrap())
        );
        let output = run_pty(&command, &[], false);
        assert!(output.contains("TERMINAL_RESTORED status=101"));
        if mode == "panic" {
            assert!(
                output.find("\x1b[?1049l").unwrap()
                    < output.find("intentional prototype panic").unwrap()
            );
        }
    }
}

#[test]
fn session_probe() -> io::Result<()> {
    let Ok(mode) = std::env::var("MYPOWERS_CLEANUP_PROBE") else {
        return Ok(());
    };
    let _session = terminal::Session::enter()?;
    if mode == "panic" {
        panic!("intentional prototype panic");
    }
    Err(io::Error::other("intentional prototype I/O error"))
}
