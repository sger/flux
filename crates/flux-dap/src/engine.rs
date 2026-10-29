//! The program under debug: a `flux debug --engine` child process.
//!
//! The adapter listens on a localhost port and starts the child, which
//! connects back and speaks [`flux::debug_protocol`]. The child's stdout and
//! stderr, the program's output, are read on threads of their own. Every
//! thread hands what it reads to the session as an [`Input`].

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flux::debug_protocol::{EngineMessage, Request, RequestMessage};

use crate::server::Input;

/// How to start the program.
#[derive(Debug, Clone, Default)]
pub struct LaunchSpec {
    /// The command that runs `flux`: `["flux"]`, or `["cargo", "run", "--"]`.
    pub flux_command: Vec<String>,
    pub program: PathBuf,
    /// Options for `flux` itself, such as `--no-cache`.
    pub flux_args: Vec<String>,
    /// Arguments for the program.
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub record: Option<PathBuf>,
    pub replay: Option<PathBuf>,
}

/// A running `flux debug --engine` child.
pub struct VmEngine {
    stream: TcpStream,
    child: Arc<Mutex<Child>>,
    next_id: u64,
}

impl VmEngine {
    /// Starts the child and waits for it to connect. Returns `None` when it
    /// exits first, for example because the program does not compile: its
    /// output has then gone to the session, followed by [`Input::Exited`].
    pub fn launch(spec: &LaunchSpec, inputs: Sender<Input>) -> Result<Option<Self>, String> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|err| format!("cannot open a port for the debugger: {err}"))?;
        let address = listener
            .local_addr()
            .map_err(|err| format!("cannot open a port for the debugger: {err}"))?;
        let (program, base) =
            spec.flux_command.split_first().ok_or_else(|| "`fluxCommand` is empty".to_string())?;
        let mut command = Command::new(program);
        command
            .args(base)
            .arg("debug")
            .arg(&spec.program)
            .arg("--engine")
            .arg(address.to_string())
            .args(&spec.flux_args);
        if let Some(tape) = &spec.record {
            command.arg("--record").arg(tape);
        }
        if let Some(tape) = &spec.replay {
            command.arg("--replay").arg(tape);
        }
        if !spec.args.is_empty() {
            command.arg("--").args(&spec.args);
        }
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        command
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child =
            command.spawn().map_err(|err| format!("cannot start `{program}`: {err}"))?;

        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let readers =
            vec![forward_stdout(stdout, inputs.clone()), forward_stderr(stderr, inputs.clone())];
        let child = Arc::new(Mutex::new(child));
        watch_exit(child.clone(), readers, inputs.clone());

        // Wait for the child to connect, or to exit without connecting.
        listener
            .set_nonblocking(true)
            .map_err(|err| format!("cannot wait for the debugger: {err}"))?;
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    let exited = child
                        .lock()
                        .map(|mut child| matches!(child.try_wait(), Ok(Some(_))))
                        .unwrap_or(true);
                    if exited {
                        return Ok(None);
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(err) => return Err(format!("cannot accept the debugger: {err}")),
            }
        };
        stream
            .set_nonblocking(false)
            .map_err(|err| format!("cannot talk to the debugger: {err}"))?;
        read_engine(
            stream.try_clone().map_err(|err| format!("cannot talk to the debugger: {err}"))?,
            inputs,
        );
        Ok(Some(Self { stream, child, next_id: 1 }))
    }

    /// Sends a request; the answer comes back as an [`Input::Engine`] with
    /// the id returned here.
    pub fn send(&mut self, request: Request) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let line = serde_json::to_string(&RequestMessage { id, request })
            .expect("engine requests serialize");
        // A child that has gone is reported by its exit, not here.
        let _ = writeln!(self.stream, "{line}").and_then(|_| self.stream.flush());
        id
    }

    /// Ends the child now.
    pub fn kill(&self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

fn read_engine(stream: TcpStream, inputs: Sender<Input>) {
    std::thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else {
                break;
            };
            match serde_json::from_str::<EngineMessage>(&line) {
                Ok(message) => {
                    if inputs.send(Input::Engine(message)).is_err() {
                        return;
                    }
                }
                Err(err) => eprintln!("flux-dap: unreadable message from the engine: {err}"),
            }
        }
    });
}

/// The program's stdout, as it comes: it may print without newlines.
fn forward_stdout(
    mut stdout: impl Read + Send + 'static,
    inputs: Sender<Input>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match stdout.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let text = String::from_utf8_lossy(&buffer[..read]).into_owned();
                    if inputs.send(Input::Output { category: "stdout", text }).is_err() {
                        break;
                    }
                }
            }
        }
    })
}

/// The child's stderr, by line, without the compiler's progress lines.
fn forward_stderr(
    stderr: impl Read + Send + 'static,
    inputs: Sender<Input>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if is_progress_line(&line) {
                        continue;
                    }
                    if inputs
                        .send(Input::Output { category: "stderr", text: line.clone() })
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    })
}

/// `[ 3 of 18] Compiling Flow.Eq` and the like.
pub fn is_progress_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with('[')
        && ["Compiling", "Cached", "Linking", "Running via"].iter().any(|word| line.contains(word))
}

/// Reports the child's exit once all its output has been read.
fn watch_exit(
    child: Arc<Mutex<Child>>,
    readers: Vec<std::thread::JoinHandle<()>>,
    inputs: Sender<Input>,
) {
    std::thread::spawn(move || {
        let code = loop {
            let status = child.lock().ok().and_then(|mut child| child.try_wait().ok());
            match status {
                Some(Some(status)) => break status.code(),
                Some(None) => std::thread::sleep(Duration::from_millis(20)),
                None => break None,
            }
        };
        for reader in readers {
            let _ = reader.join();
        }
        let _ = inputs.send(Input::Exited(code));
    });
}
