//! dd and install (nanocode's addition), after GNU coreutils. dd: if= of= bs= ibs= obs=
//! count= skip= seek= conv=notrunc status=none|noxfer, with GNU's two lines of records
//! and its summary. install: -d, -m, -D, -t, -T, -v, -p (owners are left alone).

use psh_engine::host::{FileKind, OpenOptions, error_text};
use psh_engine::util::UtilCtx;

use crate::common::{self, Input};
use crate::fsutil::join;

fn bytes_text(n: u64) -> String {
    // GNU's "1.0 kB, 1.0 KiB" forms: one decimal below ten, to the nearest.
    let scaled = |base: f64, units: &[&str]| -> String {
        let mut value = n as f64;
        let mut unit = 0;
        while value >= base && unit + 1 < units.len() {
            value /= base;
            unit += 1;
        }
        if value < 10.0 {
            format!("{value:.1} {}", units[unit])
        } else {
            format!("{value:.0} {}", units[unit])
        }
    };
    let si = scaled(1000.0, &["B", "kB", "MB", "GB", "TB", "PB"]);
    let iec = scaled(1024.0, &["B", "KiB", "MiB", "GiB", "TiB", "PiB"]);
    if n < 1000 {
        format!("{n} byte{}", if n == 1 { "" } else { "s" })
    } else if n < 1024 {
        format!("{n} bytes ({si})")
    } else {
        format!("{n} bytes ({si}, {iec})")
    }
}

pub async fn dd(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let mut input: Option<Vec<u8>> = None;
    let mut output: Option<Vec<u8>> = None;
    let (mut ibs, mut obs) = (512u64, 512u64);
    let mut count: Option<u64> = None;
    let (mut skip, mut seek) = (0u64, 0u64);
    let mut truncate = true;
    let mut status = b"default".to_vec();
    for arg in &argv[1..] {
        let Some(eq) = arg.iter().position(|&b| b == b'=') else {
            let shown = common::quote(ctx, arg);
            return common::usage(ctx, &format!("unrecognized operand {shown}")).await;
        };
        let (key, value) = (&arg[..eq], &arg[eq + 1..]);
        let size = |value: &[u8]| common::parse_count(value).filter(|n| *n > 0);
        let bad = |ctx: &UtilCtx<'_>| common::quote(ctx, value);
        match key {
            b"if" => input = Some(value.to_vec()),
            b"of" => output = Some(value.to_vec()),
            b"bs" | b"ibs" | b"obs" => {
                let Some(n) = size(value) else {
                    let v = bad(ctx);
                    return common::usage(ctx, &format!("invalid number: {v}")).await;
                };
                if key != b"obs" {
                    ibs = n;
                }
                if key != b"ibs" {
                    obs = n;
                }
            }
            b"count" | b"skip" | b"seek" => {
                let Some(n) = common::parse_count(value) else {
                    let v = bad(ctx);
                    return common::usage(ctx, &format!("invalid number: {v}")).await;
                };
                match key {
                    b"count" => count = Some(n),
                    b"skip" => skip = n,
                    _ => seek = n,
                }
            }
            b"conv" => {
                for flag in value.split(|&b| b == b',') {
                    match flag {
                        b"notrunc" => truncate = false,
                        b"fsync" | b"fdatasync" | b"noerror" => {}
                        _ => {
                            if let Some(status) = ctx.run_program(argv.to_vec()).await {
                                return status;
                            }
                            let v = common::quote(ctx, flag);
                            return common::usage(ctx, &format!("invalid conversion: {v}")).await;
                        }
                    }
                }
            }
            b"status" => status = value.to_vec(),
            b"iflag" | b"oflag" => {
                if let Some(status) = ctx.run_program(argv.to_vec()).await {
                    return status;
                }
                let v = common::quote(ctx, arg);
                return common::usage(ctx, &format!("unrecognized operand {v}")).await;
            }
            _ => {
                let shown = common::quote(ctx, arg);
                return common::usage(ctx, &format!("unrecognized operand {shown}")).await;
            }
        }
    }
    let started = ctx.now();
    let source = input.clone().unwrap_or_else(|| b"-".to_vec());
    let file = match Input::open(ctx, &source).await {
        Ok(file) => file,
        Err(e) => {
            ctx.error(&format!(
                "failed to open '{}': {}",
                String::from_utf8_lossy(&source),
                error_text(&e)
            ))
            .await;
            return 1;
        }
    };
    // Blocks as dd reads them: one read each, which may come up short.
    let mut data = Vec::new();
    let (mut full_in, mut part_in) = (0u64, 0u64);
    let mut block = vec![0u8; ibs as usize];
    let mut skipped = 0;
    loop {
        if count.is_some_and(|c| full_in + part_in >= c) {
            break;
        }
        let mut got = 0;
        // A device and a file give whole blocks; a pipe gives what it has.
        while got < block.len() {
            match file.read(ctx, &mut block[got..]).await {
                Ok(0) => break,
                Ok(n) => {
                    got += n;
                    if matches!(file, Input::Stdin) {
                        break;
                    }
                }
                Err(e) => {
                    ctx.error(&format!(
                        "error reading '{}': {}",
                        String::from_utf8_lossy(&source),
                        error_text(&e)
                    ))
                    .await;
                    return 1;
                }
            }
        }
        if got == 0 {
            break;
        }
        if skipped < skip {
            skipped += 1;
            continue;
        }
        if got == block.len() {
            full_in += 1;
        } else {
            part_in += 1;
        }
        data.extend_from_slice(&block[..got]);
        ctx.check().await;
    }
    let total = data.len() as u64;
    let (full_out, part_out) = (total / obs, u64::from(!total.is_multiple_of(obs)));
    match &output {
        None => {
            if let Err(e) = ctx.write(&data).await {
                return common::write_failed(ctx, &e).await;
            }
        }
        Some(target) => {
            let offset = (seek * obs) as usize;
            let mut whole = Vec::new();
            if (offset > 0 || !truncate)
                && let Ok(old) = Input::open(ctx, target).await
            {
                let _ = old.read_to_end(ctx, &mut whole).await;
            }
            if truncate {
                whole.truncate(offset);
            }
            if whole.len() < offset {
                whole.resize(offset, 0);
            }
            let end = offset + data.len();
            if whole.len() < end {
                whole.resize(end, 0);
            }
            whole[offset..end].copy_from_slice(&data);
            let written = match ctx.open(target, OpenOptions::write()).await {
                Ok(file) => file.write_all(&whole).await,
                Err(e) => Err(e),
            };
            if let Err(e) = written {
                ctx.error(&format!(
                    "failed to open '{}': {}",
                    String::from_utf8_lossy(target),
                    error_text(&e)
                ))
                .await;
                return 1;
            }
        }
    }
    if status != b"none" {
        let mut report =
            format!("{full_in}+{part_in} records in\n{full_out}+{part_out} records out\n");
        if status != b"noxfer" {
            let secs = ctx.now().saturating_sub(started).as_secs_f64();
            let rate = if secs > 0.0 { total as f64 / secs } else { 0.0 };
            let mut value = rate;
            let mut unit = 0;
            let units = ["B/s", "kB/s", "MB/s", "GB/s", "TB/s"];
            while value >= 1000.0 && unit + 1 < units.len() {
                value /= 1000.0;
                unit += 1;
            }
            let speed = if total == 0 || secs == 0.0 {
                "0.0 kB/s".to_string()
            } else if value < 10.0 {
                format!("{value:.1} {}", units[unit])
            } else {
                format!("{value:.0} {}", units[unit])
            };
            let secs_text = if secs == 0.0 {
                "0 s".to_string()
            } else {
                // GNU prints the seconds with %g.
                let digits = (5 - secs.log10().floor() as i32).clamp(0, 9) as usize;
                let text = format!("{secs:.digits$}");
                format!("{} s", text.trim_end_matches('0').trim_end_matches('.'))
            };
            report.push_str(&format!(
                "{} copied, {secs_text}, {speed}\n",
                bytes_text(total)
            ));
        }
        ctx.error_raw(report.as_bytes()).await;
    }
    0
}

async fn make_dirs(ctx: &mut UtilCtx<'_>, path: &[u8], mode: u32, verbose: bool) -> bool {
    let mut at = Vec::new();
    let parts: Vec<&[u8]> = path.split(|&b| b == b'/').collect();
    for (n, part) in parts.iter().enumerate() {
        if n > 0 {
            at.push(b'/');
        }
        at.extend_from_slice(part);
        if part.is_empty() || ctx.stat(&at, true).await.is_ok() {
            continue;
        }
        let last = n + 1 == parts.len();
        let dir_mode = if last { mode } else { 0o755 };
        if let Err(e) = ctx.mkdir(&at, dir_mode).await {
            ctx.error(&format!(
                "cannot create directory '{}': {}",
                String::from_utf8_lossy(&at),
                error_text(&e)
            ))
            .await;
            return false;
        }
        let _ = ctx.set_mode(&at, dir_mode).await;
        if verbose {
            let line = format!(
                "install: creating directory '{}'\n",
                String::from_utf8_lossy(&at)
            );
            let _ = ctx.write(line.as_bytes()).await;
        }
    }
    true
}

pub async fn install(ctx: &mut UtilCtx<'_>, argv: &[Vec<u8>]) -> i32 {
    let (mut dirs, mut parents, mut verbose, mut keep_times, mut no_target_dir) =
        (false, false, false, false, false);
    let mut mode = 0o755u32;
    let mut target_dir: Option<Vec<u8>> = None;
    let mut operands: Vec<Vec<u8>> = Vec::new();
    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        i += 1;
        if arg.len() < 2 || arg[0] != b'-' {
            operands.push(arg.clone());
            continue;
        }
        let text = String::from_utf8_lossy(arg).into_owned();
        let value = |i: &mut usize, attached: &str| -> Vec<u8> {
            if !attached.is_empty() {
                return attached.as_bytes().to_vec();
            }
            *i += 1;
            argv.get(*i - 1).cloned().unwrap_or_default()
        };
        let set_mode =
            |v: &[u8]| -> Option<u32> { u32::from_str_radix(std::str::from_utf8(v).ok()?, 8).ok() };
        if text == "--" {
            operands.extend(argv[i..].iter().cloned());
            break;
        } else if let Some(v) = text.strip_prefix("--mode=") {
            match set_mode(v.as_bytes()) {
                Some(m) => mode = m,
                None => return common::usage(ctx, &format!("invalid mode '{v}'")).await,
            }
        } else if let Some(v) = text.strip_prefix("--target-directory=") {
            target_dir = Some(v.as_bytes().to_vec());
        } else if matches!(text.as_str(), "--directory") {
            dirs = true;
        } else if matches!(text.as_str(), "--verbose") {
            verbose = true;
        } else if matches!(text.as_str(), "--preserve-timestamps") {
            keep_times = true;
        } else if matches!(text.as_str(), "--no-target-directory") {
            no_target_dir = true;
        } else if text.starts_with("--owner")
            || text.starts_with("--group")
            || text == "--compare"
            || text == "--strip"
        {
        } else if text.starts_with("--") {
            return common::usage(ctx, &format!("unrecognized option '{text}'")).await;
        } else {
            let letters = text.as_bytes();
            let mut k = 1;
            while k < letters.len() {
                let c = letters[k];
                k += 1;
                let rest = text[k..].to_string();
                match c {
                    b'd' => dirs = true,
                    b'D' => parents = true,
                    b'v' => verbose = true,
                    b'p' => keep_times = true,
                    b'T' => no_target_dir = true,
                    b'c' | b'C' | b's' => {}
                    b'm' => {
                        let v = value(&mut i, &rest);
                        match set_mode(&v) {
                            Some(m) => mode = m,
                            None => {
                                let v = common::quote(ctx, &v);
                                return common::usage(ctx, &format!("invalid mode {v}")).await;
                            }
                        }
                        break;
                    }
                    b't' => {
                        target_dir = Some(value(&mut i, &rest));
                        break;
                    }
                    b'o' | b'g' => {
                        value(&mut i, &rest);
                        break;
                    }
                    _ => {
                        return common::usage(ctx, &format!("invalid option -- '{}'", c as char))
                            .await;
                    }
                }
            }
        }
    }
    if dirs {
        if operands.is_empty() {
            return common::usage(ctx, "missing file operand").await;
        }
        let mut status = 0;
        for dir in &operands {
            let dir = if dir.len() > 1 {
                dir.strip_suffix(b"/").unwrap_or(dir)
            } else {
                dir
            };
            if !make_dirs(ctx, dir, mode, verbose).await {
                status = 1;
            }
        }
        return status;
    }
    let (sources, dest, into_dir) = match (&target_dir, operands.as_slice()) {
        (Some(dir), sources) if !sources.is_empty() => (sources.to_vec(), dir.clone(), true),
        (None, [source, dest]) => {
            let is_dir = !no_target_dir
                && ctx
                    .stat(dest, true)
                    .await
                    .is_ok_and(|m| m.kind == FileKind::Dir);
            (vec![source.clone()], dest.clone(), is_dir)
        }
        (None, [sources @ .., dest]) if sources.len() > 1 => (sources.to_vec(), dest.clone(), true),
        (None, [only]) => {
            let only = common::quote(ctx, only);
            return common::usage(
                ctx,
                &format!("missing destination file operand after {only}"),
            )
            .await;
        }
        _ => return common::usage(ctx, "missing file operand").await,
    };
    if into_dir {
        if parents && target_dir.is_some() && !make_dirs(ctx, &dest, 0o755, verbose).await {
            return 1;
        }
        if !ctx
            .stat(&dest, true)
            .await
            .is_ok_and(|m| m.kind == FileKind::Dir)
        {
            ctx.error(&format!(
                "target '{}' is not a directory",
                String::from_utf8_lossy(&dest)
            ))
            .await;
            return 1;
        }
    } else if parents
        && let Some(slash) = dest.iter().rposition(|&b| b == b'/')
        && slash > 0
        && !make_dirs(ctx, &dest[..slash], 0o755, verbose).await
    {
        return 1;
    }
    let mut status = 0;
    for source in &sources {
        let shown = String::from_utf8_lossy(source).into_owned();
        let target = if into_dir {
            let base = source.rsplit(|&b| b == b'/').next().unwrap_or(source);
            join(&dest, base)
        } else {
            dest.clone()
        };
        let meta = match ctx.stat(source, true).await {
            Ok(meta) if meta.kind != FileKind::Dir => meta,
            Ok(_) => {
                ctx.error(&format!("omitting directory '{shown}'")).await;
                status = 1;
                continue;
            }
            Err(e) => {
                ctx.error(&format!("cannot stat '{shown}': {}", error_text(&e)))
                    .await;
                status = 1;
                continue;
            }
        };
        let mut data = Vec::new();
        let read = match Input::open(ctx, source).await {
            Ok(input) => input.read_to_end(ctx, &mut data).await,
            Err(e) => Err(e),
        };
        if let Err(e) = read {
            ctx.error(&format!(
                "cannot open '{shown}' for reading: {}",
                error_text(&e)
            ))
            .await;
            status = 1;
            continue;
        }
        let _ = ctx.remove_file(&target).await;
        let options = OpenOptions {
            mode: Some(mode),
            ..OpenOptions::write()
        };
        let written = match ctx.open(&target, options).await {
            Ok(file) => file.write_all(&data).await,
            Err(e) => Err(e),
        };
        if let Err(e) = written {
            ctx.error(&format!(
                "cannot create regular file '{}': {}",
                String::from_utf8_lossy(&target),
                error_text(&e)
            ))
            .await;
            status = 1;
            continue;
        }
        let _ = ctx.set_mode(&target, mode).await;
        if keep_times {
            let _ = ctx
                .set_times(&target, Some(meta.mtime_ns), Some(meta.mtime_ns), true)
                .await;
        }
        if verbose {
            let line = format!("'{shown}' -> '{}'\n", String::from_utf8_lossy(&target));
            let _ = ctx.write(line.as_bytes()).await;
        }
    }
    status
}
