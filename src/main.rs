use clap::Parser;
use popol;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::process;
use std::time::Duration;
use termcolor::{Color, ColorChoice, ColorSpec, WriteColor};

// FIXME UNIX only (is popol UNIX only?)
use std::os::unix::process::ExitStatusExt;

#[derive(Debug, clap::Parser)]
#[clap(version, about)]
struct Params {
    /// Executable to run
    #[clap()]
    command: OsString,
    /// Timeout on individual reads (e.g. "1s", "1h", or "30ms")
    #[clap(long, name="duration", parse(try_from_str = duration_str::parse))]
    idle_timeout: Option<Duration>,
}

fn main() {
    if let Err(error) = cli(Params::parse()) {
        eprintln!("Error: {:#}", error);
        process::exit(1);
    }
}

fn cli(params: Params) -> anyhow::Result<()> {
    let mut child = process::Command::new(params.command)
        .stdout(process::Stdio::piped())
        .stderr(process::Stdio::piped())
        .spawn()?;

    let mut buffer = [0; 1024]; // FIXME: best buffer size?
    let mut sources = popol::Sources::with_capacity(2);
    let mut events = popol::Events::new();

    let mut child_out = child.stdout.take().expect("child.stdout is None");
    sources.register(1, &child_out, popol::interest::READ);

    let mut child_err = child.stderr.take().expect("child.stderr is None");
    sources.register(2, &child_err, popol::interest::READ);

    // FIXME? check if it’s a TTY?
    let mut stdout = termcolor::StandardStream::stdout(ColorChoice::Auto);
    let mut stderr = termcolor::StandardStream::stderr(ColorChoice::Auto);
    let mut err_color = ColorSpec::new();
    err_color.set_fg(Some(Color::Red));
    err_color.set_intense(true);

    // FIXME this sometimes messes up the order if stderr and stdout are used
    // in the same line. Not sure this is possible to fix.
    'outer: loop {
        wait_on(&mut sources, &mut events, params.idle_timeout)?;

        for (key, event) in events.iter() {
            // FIXME does read ever return non-zero if event.hangup?
            if event.readable || event.hangup {
                loop {
                    let count = if *key == 1 {
                        child_out.read(&mut buffer)?
                    } else {
                        child_err.read(&mut buffer)?
                    };

                    if count == 0 {
                        // FIXME detect actual EOF, or SIGCHILD?
                        break 'outer;
                    }

                    if *key == 2 {
                        stderr.set_color(&err_color)?;
                        stderr.write_all(&buffer[..count])?;
                        stderr.reset()?;
                        stderr.flush()?; // Probably not necessary.
                    } else {
                        stdout.write_all(&buffer[..count])?;
                        stdout.flush()?; // If there wasn’t a newline.
                    }

                    if count < buffer.len() {
                        break;
                    }
                }
            }
        }
    }

    let status = child.wait().expect("failed to wait on child");
    process::exit(
        wait_status_to_code(status).expect("no exit code or signal for child"),
    );
}

fn wait_on<T>(
    sources: &mut popol::Sources<T>,
    events: &mut popol::Events<T>,
    timeout: Option<Duration>,
) -> anyhow::Result<()>
where
    T: Clone,
    T: Eq,
{
    match timeout {
        Some(timeout) => sources.wait_timeout(events, timeout),
        None => sources.wait(events),
    }
    .map_err(|e| e.into())
    // FIXME? handle if err.kind() == io::ErrorKind::TimedOut
}

/// Get the actual exit code from a finished child process
fn wait_status_to_code(status: process::ExitStatus) -> Option<i32> {
    status.code().or_else(|| Some(128 + status.signal()?))
}
