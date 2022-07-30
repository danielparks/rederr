use clap::Parser;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::unix::prelude::AsRawFd;
use std::os::unix::process::ExitStatusExt;
use std::process;
use std::time::Duration;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

#[derive(Debug, Parser)]
#[clap(version, about, allow_hyphen_values = true, trailing_var_arg = true)]
struct Params {
    /// The executable to run
    #[clap()]
    command: OsString,

    /// Arguments to pass to the executable
    #[clap()]
    args: Vec<OsString>,

    /// Always output in color
    #[clap(long, short = 'c')]
    always_color: bool,

    /// Timeout on individual reads (e.g. "1s", "1h", or "30ms")
    #[clap(long, name="duration", parse(try_from_str = duration_str::parse))]
    idle_timeout: Option<Duration>,

    /// Don't combine stderr into stdout; keep them separate
    #[clap(long, short)]
    separate: bool,

    // Hidden: output debugging information rather than coloring stderr
    #[clap(long, hide = true)]
    debug: bool,

    /// Hidden: how large a buffer to use
    #[clap(long, default_value_t = 1024, hide = true)]
    buffer_size: usize,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum PollKey {
    Out,
    Err,
}

fn main() {
    if let Err(error) = cli(Params::parse()) {
        eprintln!("Error: {:#}", error);
        process::exit(1);
    }
}

fn cli(params: Params) -> anyhow::Result<()> {
    let mut child = process::Command::new(&params.command)
        .args(&params.args)
        .stdout(process::Stdio::piped())
        .stderr(process::Stdio::piped())
        .spawn()?;

    let mut sources = popol::Sources::with_capacity(2);
    let mut events = popol::Events::new();

    let mut child_out = child.stdout.take().expect("child.stdout is None");
    set_nonblock(&child_out)
        .expect("child stdout cannot be set to non-blocking");
    sources.register(PollKey::Out, &child_out, popol::interest::READ);

    let mut child_err = child.stderr.take().expect("child.stderr is None");
    set_nonblock(&child_err)
        .expect("child stderr cannot be set to non-blocking");
    sources.register(PollKey::Err, &child_err, popol::interest::READ);

    let mut out_out = color_stream(atty::Stream::Stdout, &params);
    let mut out_err = if params.separate {
        color_stream(atty::Stream::Stderr, &params)
    } else {
        color_stream(atty::Stream::Stdout, &params)
    };

    let mut err_color = ColorSpec::new();
    err_color.set_fg(Some(Color::Red));
    err_color.set_intense(true);

    let mut buffer = vec![0; params.buffer_size];

    // FIXME? this sometimes messes up the order if stderr and stdout are used
    // in the same line. Not sure this is possible to fix.
    while !sources.is_empty() {
        wait_on(&mut sources, &mut events, params.idle_timeout)?;

        for (key, event) in events.iter() {
            if params.debug {
                println!("{:?} {:?}", key, event);
            }

            if event.readable {
                loop {
                    let result = if *key == PollKey::Out {
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
                            } else {
                                return Err(err.into());
                            }
                        }
                    };

                    if params.debug {
                        // FIXME don’t require UTF-8
                        println!(
                            "read {} bytes {:?}",
                            count,
                            std::str::from_utf8(&buffer[..count]).unwrap()
                        );
                    } else if count > 0 {
                        // Only output if there’s something to output.
                        if *key == PollKey::Out {
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

            if event.hangup {
                // Remove the stream from poll.
                sources.unregister(key);
            }
        }
    }

    let status = child.wait().expect("failed to wait on child");
    process::exit(
        wait_status_to_code(status).expect("no exit code or signal for child"),
    );
}

/// Set a stream to be non-blocking
fn set_nonblock(fd: &dyn AsRawFd) -> io::Result<()> {
    let fd = fd.as_raw_fd();

    // SAFETY: required for FFI; shouldn’t break rust guarantees.
    match unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) } {
        0 => Ok(()),
        -1 => Err(io::Error::last_os_error()),
        other => panic!("fcntl returned {} instead of 0 or -1", other),
    }
}

fn color_stream(stream: atty::Stream, params: &Params) -> StandardStream {
    let choice = if params.always_color {
        ColorChoice::Always
    } else if atty::is(stream) {
        ColorChoice::Auto
    } else {
        ColorChoice::Never
    };

    match stream {
        atty::Stream::Stdout => StandardStream::stdout(choice),
        atty::Stream::Stderr => StandardStream::stderr(choice),
        atty::Stream::Stdin => panic!("can't output to stdin"),
    }
}

fn wait_on(
    sources: &mut popol::Sources<PollKey>,
    events: &mut popol::Events<PollKey>,
    timeout: Option<Duration>,
) -> anyhow::Result<()> {
    // FIXME? handle EINTR? I don’t think it will come up unless we have a
    // signal handler set.
    match timeout {
        Some(timeout) => sources.wait_timeout(events, timeout),
        None => sources.wait(events),
    }
    .map_err(|e| e.into())
    // FIXME better message if err.kind() == io::ErrorKind::TimedOut
}

/// Get the actual exit code from a finished child process
fn wait_status_to_code(status: process::ExitStatus) -> Option<i32> {
    status.code().or_else(|| Some(128 + status.signal()?))
}
