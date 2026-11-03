//! The `atvprep` binary; started as `atvprep-hook` it is the Servarr hook.

use std::ffi::OsString;
use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;

use atvprep::{Environment, Io, hook_main, run};
mod manager_main;

fn is_hook(program: Option<&OsString>) -> bool {
    program
        .map(Path::new)
        .and_then(Path::file_stem)
        .is_some_and(|name| name == "atvprep-hook")
}

fn is_manager(program: Option<&OsString>) -> bool {
    program
        .map(Path::new)
        .and_then(Path::file_stem)
        .is_some_and(|name| name == "atvprep-manager")
}

#[cfg(target_os = "macos")]
fn enter_manager_directory(
    args: &[OsString],
) -> Result<(), atvprep_core::fd_fix_contract::UnsafeFixTarget> {
    let arguments = std::iter::once(OsString::from("atvprep")).chain(args.iter().cloned());
    let Ok(matches) = atvprep::args::command().try_get_matches_from(arguments) else {
        return Ok(());
    };
    let Some(("prepare" | "fix" | "convert", options)) = matches.subcommand() else {
        return Ok(());
    };
    let Some(fd) = options.get_one::<i128>("manager-parent-fd") else {
        return Ok(());
    };
    if ![
        "manager-leaf",
        "manager-device",
        "manager-inode",
        "manager-ctime-ns",
    ]
    .iter()
    .all(|key| options.contains_id(key))
    {
        return Ok(());
    }
    let fd = i32::try_from(*fd).map_err(|_| {
        atvprep_core::fd_fix_contract::UnsafeFixTarget("invalid parent descriptor".to_owned())
    })?;
    atvprep_core::fd_fix_contract::enter_inherited_directory(fd)
}

fn main() -> ExitCode {
    let mut all = std::env::args_os();
    let program = all.next();
    let hook = is_hook(program.as_ref());
    let manager = is_manager(program.as_ref());
    let mut args: Vec<OsString> = all.collect();
    if hook && args.len() == 1 && args[0] == "--version" {
        println!("atvprep-hook {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if manager || (!hook && args.first().is_some_and(|value| value == "manager")) {
        if !manager {
            args.remove(0);
        }
        return ExitCode::from(manager_main::run(&args));
    }
    #[cfg(target_os = "macos")]
    if !hook && let Err(error) = enter_manager_directory(&args) {
        eprintln!("Error: {error}");
        return ExitCode::FAILURE;
    }
    let env = Environment::from_process();
    let code = if hook {
        hook_main::run(
            &args,
            &env,
            &mut std::io::stdout(),
            Box::new(std::io::stderr()),
        )
    } else {
        let io = Io {
            stdin: Box::new(std::io::BufReader::new(std::io::stdin())),
            stdout: Box::new(std::io::stdout()),
            stderr: Box::new(std::io::stderr()),
            stdout_is_terminal: std::io::stdout().is_terminal(),
            stdin_is_terminal: std::io::stdin().is_terminal(),
        };
        run(&args, &env, io)
    };
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
