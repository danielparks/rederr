use clap::Parser;
use popol::set_nonblocking;
use std::io::{self, Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::process;
use termcolor::{Color, ColorChoice, ColorSpec, StandardStream, WriteColor};

mod params;
use params::Params;

#[derive(Clone, PartialEq, Eq, Debug)]
enum PollKey {
    Out,
    Err,
}

macro_rules! fail {
    ($($arg:tt)*) => {
        eprintln!($($arg)*);
        process::exit(1);
    };
}

fn main() {
    if let Err(error) = cli(Params::parse()) {
        fail!("Error: {:#}", error);
    }
}

fn cli(params: Params) -> anyhow::Result<()> {
    let mut child = process::Command::new(&params.command)
        .args(&params.args)
        .stdout(process::Stdio::piped())
        .stderr(process::Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| {
            fail!("Could not run command {:?}: {}", params.command, err);
        });

    let mut sources = popol::Sources::with_capacity(2);
    let mut events = Vec::with_capacity(2);

    let mut child_out = child.stdout.take().expect("child.stdout is None");
    set_nonblocking(&child_out, true)
        .expect("child stdout cannot be set to non-blocking");
    sources.register(PollKey::Out, &child_out, popol::interest::READ);

    let mut child_err = child.stderr.take().expect("child.stderr is None");
    set_nonblocking(&child_err, true)
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
        // FIXME? handle EINTR? I don’t think it will come up unless we have a
        // signal handler set.
        sources
            .poll(&mut events, params.idle_timeout)
            .unwrap_or_else(|err| {
                if err.kind() == io::ErrorKind::TimedOut {
                    if let Some(timeout) = params.idle_timeout {
                        fail!(
                            "Timed out waiting for input after {:?}",
                            timeout
                        );
                    }
                }

                fail!("Error while waiting for input: {:#}", err);
            });

        for event in events.drain(..) {
            if params.debug {
                println!("{:?}", event);
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

/// Get the actual exit code from a finished child process
fn wait_status_to_code(status: process::ExitStatus) -> Option<i32> {
    status.code().or_else(|| Some(128 + status.signal()?))
}
