//! `tee`: `-a`, `-i` (accepted; the shell owns signals), `-p` and `--output-error`
//! (a closed stdout does not stop the copy to the files). `-` is a file name, as in GNU
//! and BSD. Written for psh after GNU's documented behavior.

use psh_engine::host::{OpenOptions, error_text};
use psh_engine::util::{UtilCtx, UtilFile};

use crate::common;
use crate::opts::{Spec, both, flag, long};

pub const SPEC: Spec = Spec {
    name: "tee",
    opts: &[
        both('a', "append"),
        both('i', "ignore-interrupts"),
        flag('p'),
        long("output-error").optional(),
    ],
    posix_order: false,
};

pub async fn run(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let p = match common::parse(ctx, &SPEC, argv).await {
        Ok(p) => p,
        Err(st) => return st,
    };
    let append = p.has(&SPEC, "append");
    // With -p or --output-error, a closed stdout is reported (warn) or ignored, and the
    // files keep receiving; without, it ends tee as SIGPIPE would.
    let mut keep_on_pipe = p.has(&SPEC, "p");
    let mut warn_pipe = false;
    for (key, value) in p.iter(&SPEC) {
        if key == "output-error" {
            keep_on_pipe = true;
            match value.unwrap_or(b"warn-nopipe") {
                b"warn" | b"exit" => warn_pipe = true,
                b"warn-nopipe" | b"exit-nopipe" => {}
                v => {
                    let v = common::quote(ctx, v);
                    let opt = common::quote(ctx, b"--output-error");
                    return common::usage(ctx, &format!("invalid argument {v} for {opt}")).await;
                }
            }
        }
    }
    let mut status = 0;
    let mut files: Vec<(Vec<u8>, Option<UtilFile>)> = Vec::new();
    for name in &p.operands {
        let opts = if append {
            OpenOptions::append()
        } else {
            OpenOptions::write()
        };
        match ctx.open(name, opts).await {
            Ok(f) => files.push((name.clone(), Some(f))),
            Err(e) => {
                common::file_error(ctx, name, &e).await;
                status = 1;
            }
        }
    }
    let mut stdout_open = true;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match ctx.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                ctx.error(&format!("read error: {}", error_text(&e))).await;
                status = 1;
                break;
            }
        };
        let data = &buf[..n];
        if stdout_open {
            // Written through at once: tee is how agents watch a long command.
            let r = match ctx.write(data).await {
                Ok(()) => ctx.flush().await,
                Err(e) => Err(e),
            };
            if let Err(e) = r {
                let pipe = e.kind() == std::io::ErrorKind::BrokenPipe;
                if !pipe || !keep_on_pipe {
                    return common::write_failed(ctx, &e).await;
                }
                if warn_pipe {
                    ctx.error(&format!("standard output: {}", error_text(&e)))
                        .await;
                    status = 1;
                }
                stdout_open = false;
            }
        }
        for (name, file) in files.iter_mut() {
            if let Some(f) = file
                && let Err(e) = f.write_all(data).await
            {
                common::file_error(ctx, name, &e).await;
                status = 1;
                *file = None;
            }
        }
        if !stdout_open && files.iter().all(|(_, f)| f.is_none()) {
            break;
        }
        ctx.check().await;
    }
    status
}
