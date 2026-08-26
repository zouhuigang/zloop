use clap::Parser;
use zloop::cli;

/// Restore default SIGPIPE handling so `zloop status | head` exits quietly instead of panicking.
#[cfg(unix)]
fn reset_sigpipe() {
    extern "C" {
        fn signal(sig: i32, handler: usize) -> usize;
    }
    const SIGPIPE: i32 = 13;
    const SIG_DFL: usize = 0;
    unsafe {
        signal(SIGPIPE, SIG_DFL);
    }
}

#[cfg(not(unix))]
fn reset_sigpipe() {}

fn main() {
    reset_sigpipe();
    let args = cli::Cli::parse();
    let code = match cli::run(args) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("zloop: {err}");
            cli::exit_code_for(&err)
        }
    };
    std::process::exit(code);
}
