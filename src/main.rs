use popol;
use simplelog::{
    ColorChoice, CombinedLogger, Config, ConfigBuilder, LevelFilter,
    TermLogger, TerminalMode,
};
use std::io;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time;
use structopt::StructOpt;

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

    'outer: loop {
        match sources.wait_timeout(&mut events, time::Duration::from_secs(6)) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::TimedOut => {
                std::process::exit(1)
            }
            Err(err) => return Err(err.into()),
        }

        for (key, event) in events.iter() {
            // FIXME does read ever return non-zero if event.hangup?
            //println!("event: {:?}", event);
            if event.readable || event.hangup {
                print!("{}: ", if *key == 1 { "out" } else { "err" });
                loop {
                    let bytes = if *key == 1 {
                        child_out.read(&mut buffer)?
                    } else {
                        child_err.read(&mut buffer)?
                    };

                    if bytes == 0 {
                        // FIXME detect actual EOF, or SIGCHILD?
                        eprintln!("GOT BYTES == 0");
                        break 'outer;
                    }

                    print!("{:?}", std::str::from_utf8(&buffer[..bytes])?);
                    //io::stdout().write_all(&buf[..n])?

                    if bytes < buffer.len() {
                        eprintln!(
                            "bytes < buffer.len(): {} < {}",
                            bytes,
                            buffer.len()
                        );
                        break;
                    }
                }
                println!(">");
            }
        }
    }

    let exit_code = child.wait().expect("failed to wait on child");

    println!("exit code: {:?}", exit_code);

    Ok(())
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
