use std::io::{BufReader, Write};
use std::sync::mpsc::channel;

use flux_dap::transport::{read_message, write_message};
use flux_dap::{Input, Session};

fn main() {
    let log = std::env::var_os("FLUX_DAP_LOG").is_some();
    let (sender, inputs) = channel();

    // The editor's requests, read on a thread of their own so the session
    // also hears the program while it waits.
    let from_editor = sender.clone();
    std::thread::spawn(move || {
        let mut stdin = BufReader::new(std::io::stdin());
        while let Ok(Some(message)) = read_message(&mut stdin) {
            if log {
                eprintln!("flux-dap <- {message}");
            }
            if from_editor.send(Input::Client(message)).is_err() {
                break;
            }
        }
    });

    let mut stdout = std::io::stdout();
    Session::new(inputs, sender, move |message| {
        if log {
            eprintln!("flux-dap -> {message}");
        }
        let _ = write_message(&mut stdout, &message);
    })
    .run();
    let _ = std::io::stdout().flush();
}
