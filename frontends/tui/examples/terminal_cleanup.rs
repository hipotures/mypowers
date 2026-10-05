//! Subprocess fixture for testing the production terminal guard in an isolated PTY.
#[path = "../src/terminal.rs"]
mod terminal;

fn main() -> std::io::Result<()> {
    let mode = std::env::args().nth(1).unwrap();
    let _session = terminal::Session::enter(true)?;
    eprintln!("SESSION READY");
    match mode.as_str() {
        "normal" => {}
        "main-panic" => panic!("Main-thread cleanup test"),
        "worker-panic" => {
            let result = std::thread::spawn(|| panic!("Worker cleanup test")).join();
            assert!(result.is_err());
            eprintln!("WORKER PANIC SURVIVED");
        }
        "task-panic" => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()?;
            let result = runtime.block_on(async {
                tokio::spawn(async { panic!("Async-task cleanup test") }).await
            });
            assert!(result.is_err());
            eprintln!("TASK PANIC SURVIVED");
        }
        _ => panic!("Unknown cleanup test mode"),
    }
    Ok(())
}
