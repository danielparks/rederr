use popol;
use simplelog::{
    ColorChoice, CombinedLogger, Config, ConfigBuilder, LevelFilter,
    TermLogger, TerminalMode,
};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use structopt::StructOpt;

// FIXME UNIX only
use std::os::unix::process::ExitStatusExt;

#[derive(Debug, StructOpt)]
struct Params {
    /// Executable to run
    #[structopt(parse(from_os_str))]
    command: PathBuf,
    /// Verbosity (may be repeated up to three times)
    #[structopt(short, long, parse(from_occurrences))]
    verbose: u8,
}

fn main() {
    if let Err(error) = cli(Params::from_args()) {
        eprintln!("Error: {:#}", error);
        std::process::exit(1);
    }
}

fn cli(params: Params) -> anyhow::Result<()> {
    let filter = match params.verbose {
        3.. => LevelFilter::Trace,
        2 => LevelFilter::Debug,
        1 => LevelFilter::Info,
        0 => LevelFilter::Warn,
    };

    CombinedLogger::init(vec![
        // Default logger
        new_term_logger(filter, new_logger_config().build()),
    ])
    .unwrap();

    println!("command: {:?}", params.command);

    let mut child = Command::new(params.command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut buffer = [0; 500]; // FIXME: best buffer size?
    let mut sources = popol::Sources::with_capacity(2);
    let mut events = popol::Events::with_capacity(2);

    let mut child_out = child.stdout.take().expect("child.stdout is None");
    sources.register(1, &child_out, popol::interest::READ);

    let mut child_err = child.stderr.take().expect("child.stderr is None");
    sources.register(2, &child_err, popol::interest::READ);

    let mut last_key = 1;

    'outer: loop {
        // FIXME configurable timeout
        wait_on(&mut sources, &mut events, None)?;

        for (key, event) in events.iter() {
            // FIXME does read ever return non-zero if event.hangup?
            if event.readable || event.hangup {
                loop {
                    if last_key == 1 && *key == 2 {
                        print!("<")
                    } else if last_key == 2 && *key == 1 {
                        print!(">")
                    }

                    last_key = *key;

                    let count = if *key == 1 {
                        child_out.read(&mut buffer)?
                    } else {
                        child_err.read(&mut buffer)?
                    };

                    if count == 0 {
                        // FIXME detect actual EOF, or SIGCHILD?
                        break 'outer;
                    }

                    io::stdout().write_all(&buffer[..count])?;

                    if count < buffer.len() {
                        break;
                    }
                }
            }
        }
    }

    let status = child.wait().expect("failed to wait on child");
    std::process::exit(
        code_for_wait_status(status).expect("no exit code or signal for child"),
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

fn code_for_wait_status(status: std::process::ExitStatus) -> Option<i32> {
    status.code().or(Some(128 + status.signal()?))
}

fn new_term_logger(level: LevelFilter, config: Config) -> Box<TermLogger> {
    TermLogger::new(level, config, TerminalMode::Mixed, ColorChoice::Auto)
}

fn new_logger_config() -> ConfigBuilder {
    let mut builder = ConfigBuilder::new();
    builder.set_time_to_local(true);
    builder.set_target_level(LevelFilter::Error);
    builder
}
