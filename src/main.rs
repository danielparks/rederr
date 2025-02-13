//! `cron-wrapper` executable.

// Lint configuration in Cargo.toml isn’t supported by cargo-geiger.
#![forbid(unsafe_code)]

use bstr::ByteSlice;
use clap::Parser;
use popol::set_nonblocking;
use std::cmp;
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::process;
use std::time::Duration;
use termcolor::{Color, ColorSpec, WriteColor};

mod params;
use params::Params;

mod timeout;
use timeout::Timeout;

/// Key to identify child output stream that when `poll()` returns.
#[derive(Clone, PartialEq, Eq, Debug)]
enum PollKey {
    /// Child stdout stream.
    Out,

    /// Child stderr stream.
    Err,
}

/// Display an error message and exit with code 1.
macro_rules! fail {
    ($($arg:tt)*) => {{
        eprintln!($($arg)*);
        process::exit(1);
    }};
}

/// Maximum timeout that poll allows.
const POLL_MAX_TIMEOUT: Timeout =
    Timeout::Future { timeout: Duration::from_millis(i32::MAX as u64) };

fn main() {
    if let Err(error) = cli(&Params::parse()) {
        fail!("Error: {:#}", error);
    }
}

/// Initialize logging and run the child.
fn cli(params: &Params) -> anyhow::Result<()> {
    let run_timeout = Timeout::from(params.run_timeout).start();
    let idle_timeout = Timeout::from(params.idle_timeout);

    let mut child = process::Command::new(&params.command)
        .args(&params.args)
        .stdout(process::Stdio::piped())
        .stderr(process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| {
            fail!("Could not run command {:?}: {}", params.command, err);
        });

    let mut sources = popol::Sources::with_capacity(2);
    let mut events = VecDeque::with_capacity(2);

    let mut child_out = child.stdout.take().expect("child.stdout is None");
    set_nonblocking(&child_out, true)
        .expect("child stdout cannot be set to non-blocking");
    sources.register(PollKey::Out, &child_out, popol::interest::READ);

    let mut child_err = child.stderr.take().expect("child.stderr is None");
    set_nonblocking(&child_err, true)
        .expect("child stderr cannot be set to non-blocking");
    sources.register(PollKey::Err, &child_err, popol::interest::READ);

    let mut out_out = params.out_stream();
    let mut out_err = params.err_stream();

    let mut err_color = ColorSpec::new();
    err_color.set_fg(Some(Color::Red));
    err_color.set_intense(true);

    let mut buffer = vec![0; params.buffer_size];

    // FIXME? this sometimes messes up the order if stderr and stdout are used
    // in the same line. Not sure this is possible to fix.
    while !sources.is_empty() {
        let timeout = cmp::min(&run_timeout, &idle_timeout);
        if let Some(expired) = timeout.check_expired() {
            timeout_fail(timeout, &expired);
        }

        if params.debug {
            println!(
                "poll() with timeout {timeout} (run timeout {run_timeout})"
            );
        }

        match poll(&mut sources, &mut events, timeout) {
            Ok(None) => {} // Success
            Ok(Some(expired)) => timeout_fail(timeout, &expired),
            Err(error) => fail!("Error while waiting for input: {:?}", error),
        }

        while let Some(event) = events.pop_front() {
            if params.debug {
                println!("{event:?}");
            }

            if event.is_readable() {
                loop {
                    let result = if event.key == PollKey::Out {
                        child_out.read(&mut buffer)
                    } else {
                        child_err.read(&mut buffer)
                    };

                    let count = match result {
                        Ok(count) => count,
                        Err(err) => {
                            if err.kind() == io::ErrorKind::WouldBlock {
                                // Done reading.
                                if params.debug {
                                    println!("io::ErrorKind::WouldBlock");
                                }

                                break;
                            }

                            return Err(err.into());
                        }
                    };

                    if params.debug {
                        println!(
                            "read {} bytes {:?}",
                            count,
                            buffer[..count].as_bstr()
                        );
                    } else if count > 0 {
                        // Only output if there’s something to output.
                        if event.key == PollKey::Out {
                            out_out.write_all(&buffer[..count])?;
                            out_out.flush()?; // If there wasn’t a newline.
                        } else {
                            out_err.set_color(&err_color)?;
                            out_err.write_all(&buffer[..count])?;
                            out_err.reset()?;
                            out_err.flush()?; // If there wasn’t a newline.
                        }
                    }

                    if count < buffer.len() {
                        // We could read again and get either 0 bytes or
                        // io::ErrorKind::WouldBlock, but I think this check
                        // makes it more likely the output ordering is correct.
                        // A partial read indicates that the stream had stopped,
                        // so we should check to see if another stream is ready.
                        break;
                    }
                }
            }

            if event.is_hangup() {
                // Remove the stream from poll.
                sources.unregister(&event.key);
            }
        }
    }

    let status = child.wait().expect("failed to wait on child");
    process::exit(
        wait_status_to_code(status).expect("no exit code or signal for child"),
    );
}

/// Display a message about the timeout expiring.
///
/// `timeout` is the original timeout; `expired` is the timeout object after it
/// expired. You can determine the type of timeout based on the variant of
/// `timeout`, since the idle timeout is always `Timeout::Future` or
/// `Timeout::Never` and the overall run timeout is always `Timeout::Pending`
/// or `Timeout::Never`.
fn timeout_fail(timeout: &Timeout, expired: &Timeout) {
    match &timeout {
        Timeout::Never => panic!("timed out when no timeout was set"),
        Timeout::Expired { .. } => panic!("did not expect Timeout::Expired"),
        Timeout::Future { .. } => {
            fail!(
                "Timed out waiting for input after {:?}",
                expired.elapsed_rounded()
            )
        }
        Timeout::Pending { .. } => {
            fail!("Run timed out after {:?}", expired.elapsed_rounded())
        }
    }
}

/// Wait for input.
///
/// Returns:
///  * `Ok(None)`: got input.
///  * `Ok(Some(Timeout::Expired { .. })`: timeout expired without input.
///  * `Err(error)`: an error occurred.
fn poll(
    sources: &mut popol::Sources<PollKey>,
    events: &mut VecDeque<popol::Event<PollKey>>,
    timeout: &Timeout,
) -> anyhow::Result<Option<Timeout>> {
    // FIXME? handle EINTR? I don’t think it will come up unless we have a
    // signal handler set.
    let timeout = timeout.start();
    while events.is_empty() {
        if let Some(expired) = timeout.check_expired() {
            return Ok(Some(expired));
        }

        let call_timeout = cmp::min(&timeout, &POLL_MAX_TIMEOUT).timeout();
        if let Err(error) = sources.poll(events, call_timeout) {
            // Ignore valid timeouts; they are handled on next loop.
            if call_timeout.is_some() && error.kind() == io::ErrorKind::TimedOut
            {
                continue;
            }

            // Invalid timeout or other error.
            return Err(error.into());
        }
    }

    Ok(None)
}

/// Get the actual exit code from a finished child process
fn wait_status_to_code(status: process::ExitStatus) -> Option<i32> {
    // FIXME: broken on windows.
    status
        .code()
        // status.signal() shouldn’t be >32, but we use saturating_add()
        // just to be safe.
        .or_else(|| Some(status.signal()?.saturating_add(128)))
}
