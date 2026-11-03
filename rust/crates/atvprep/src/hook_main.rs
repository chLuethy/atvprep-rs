//! The hook entry point: `atvprep-hook <sourcePath> <destinationFilePath>`.

use std::ffi::OsString;
use std::io::Write;

use atvprep_core::console::Console;
use atvprep_core::hook::run_hook;
use atvprep_core::pipeline_postfix::RunContext;
use atvprep_core::stages::StageReporter;

use crate::Environment;
use crate::sink::SharedSink;

/// Run the hook: stdout gets the `[MediaFile]` line, stderr everything else (prepare's console
/// and machine lines included).
pub fn run(
    args: &[OsString],
    env: &Environment,
    stdout: &mut dyn Write,
    stderr: Box<dyn Write + Send>,
) -> i32 {
    let sink = SharedSink::new(stderr);
    let mut ctx = RunContext {
        console: Console::to(Box::new(sink.clone()), false),
        reporter: StageReporter::new(Box::new(sink), false),
        tools: (env.make_tools)(),
        ffprobe: env.ffprobe.clone(),
        ffmpeg: env.ffmpeg.clone(),
    };
    let code = run_hook(args, &mut ctx, stdout);
    ctx.reporter.finish();
    code
}
