use duga_sandbox::CancellationToken;

pub fn setup_signal_handler(cancel: CancellationToken) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_err() {
            return;
        }
        eprintln!("Cancelling... (press Ctrl+C again to force quit)");
        cancel.cancel();

        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("Force quitting.");
            std::process::exit(1);
        }
    })
}
