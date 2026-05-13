//! VM Flow.Channel integration tests.

use std::path::Path;
use std::process::Command;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_flux_source(source: &str, tag: &str) -> (String, String, bool) {
    let dir = std::env::temp_dir().join(format!(
        "flux-vm-channel-{}-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test"),
        tag
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir for Flow.Channel fixture");
    let path = dir.join("channel.flx");
    std::fs::write(&path, source).expect("write Flow.Channel fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_flux"))
        .current_dir(workspace_root())
        .args([path.to_str().unwrap(), "--no-cache"])
        .output()
        .expect("run flux on Flow.Channel fixture");

    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n");
    let _ = std::fs::remove_file(&path);
    (stdout, stderr, output.status.success())
}

fn assert_flux(source: &str, tag: &str, expected: &[&str]) {
    let (stdout, stderr, success) = run_flux_source(source, tag);
    assert!(
        success,
        "Flow.Channel fixture {tag} must succeed:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines, expected, "unexpected stdout for {tag}");
}

#[test]
fn channel_buffered_send_recv_same_fiber() {
    assert_flux(
        r#"
import Flow.Async as Async
import Flow.Channel as Channel

fn main() with IO {
    print(Async.run_async(channel_main))
}

fn channel_main() -> Int with Async {
    let ch = Channel.make(1)
    Channel.send(ch, 11)
    match Channel.recv(ch) {
        Some(v) -> v,
        None -> 0
    }
}
"#,
        "same-fiber",
        &["11"],
    );
}

#[test]
fn channel_recv_parks_until_task_sender_wakes_it() {
    assert_flux(
        r#"
import Flow.Async as Async
import Flow.Channel as Channel
import Flow.Task as Task

fn main() with IO {
    print(Async.run_async(channel_main))
}

fn channel_main() -> Int with Async {
    let ch = Channel.make(1)
    let _t = Task.spawn(fn() { Channel.try_send(ch, 23) })
    match Channel.recv(ch) {
        Some(v) -> v,
        None -> 0
    }
}
"#,
        "recv-parks",
        &["23"],
    );
}

#[test]
fn channel_task_producer_buffers_two_values() {
    assert_flux(
        r#"
import Flow.Async as Async
import Flow.Channel as Channel
import Flow.Task as Task

fn main() with IO {
    print(Async.run_async(channel_main))
}

fn channel_main() -> Int with Async {
    let ch = Channel.make(2)
    let _t = Task.spawn(fn() {
        let a = Channel.try_send(ch, 3)
        let b = Channel.try_send(ch, 4)
        a && b
    })
    let a = Channel.recv(ch)
    let b = Channel.recv(ch)
    match (a, b) {
        (Some(x), Some(y)) -> x + y,
        _ -> 0
    }
}
"#,
        "task-producer",
        &["7"],
    );
}

#[test]
fn channel_close_unblocks_pending_receiver() {
    assert_flux(
        r#"
import Flow.Async as Async
import Flow.Channel as Channel
import Flow.Task as Task

fn main() with IO {
    print(Async.run_async(channel_main))
}

fn channel_main() -> Int with Async {
    let ch = Channel.make(0)
    Channel.close(ch)
    match Channel.recv(ch) {
        Some(_) -> 1,
        None -> 2
    }
}
"#,
        "close-receiver",
        &["2"],
    );
}

#[test]
fn channel_try_send_reports_full_and_available() {
    assert_flux(
        r#"
import Flow.Channel as Channel

fn main() with IO {
    let ch = Channel.make(1)
    print(Channel.try_send(ch, 1))
    print(Channel.try_send(ch, 2))
    let _ = Channel.try_recv(ch)
    print(Channel.try_send(ch, 3))
}
"#,
        "try-send",
        &["true", "false", "true"],
    );
}

#[test]
fn channel_try_recv_reports_empty_and_buffered() {
    assert_flux(
        r#"
import Flow.Channel as Channel

fn main() with IO {
    let ch = Channel.make(1)
    match Channel.try_recv(ch) {
        Some(_) -> print("bad"),
        None -> print("empty")
    }
    let _ = Channel.try_send(ch, 9)
    match Channel.try_recv(ch) {
        Some(v) -> print(v),
        None -> print(0)
    }
}
"#,
        "try-recv",
        &["\"empty\"", "9"],
    );
}

#[test]
fn channel_handle_is_sendable_into_task_closure() {
    assert_flux(
        r#"
import Flow.Channel as Channel
import Flow.Task as Task

fn main() with IO {
    let ch = Channel.make(1)
    let t = Task.spawn(fn() { Channel.try_send(ch, 31) })
    print(Task.blocking_join(t))
    match Channel.try_recv(ch) {
        Some(v) -> print(v),
        None -> print(0)
    }
}
"#,
        "sendable-handle",
        &["true", "31"],
    );
}

#[test]
fn channel_len_and_cap_report_buffer_state() {
    assert_flux(
        r#"
import Flow.Channel as Channel

fn main() with IO {
    let ch = Channel.make(2)
    print(Channel.cap(ch))
    print(Channel.len(ch))
    let _a = Channel.try_send(ch, 1)
    let _b = Channel.try_send(ch, 2)
    print(Channel.len(ch))
}
"#,
        "len-cap",
        &["2", "0", "2"],
    );
}
