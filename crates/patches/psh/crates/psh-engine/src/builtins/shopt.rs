//! `shopt`: bash's other options (D1280).

use crate::options::{self, SET_OPTS, SHOPTS};
use crate::state::{R, Vp};

use super::{out, s};

const USAGE: &str = "shopt: usage: shopt [-pqsu] [-o] [optname ...]";

pub(crate) async fn shopt(vp: &mut Vp, args: &[Vec<u8>]) -> R {
    let (mut set, mut unset, mut print, mut quiet, mut o) = (false, false, false, false, false);
    let mut i = 1;
    while let Some(a) = args.get(i) {
        if a == b"--" {
            i += 1;
            break;
        }
        if a.len() < 2 || a[0] != b'-' {
            break;
        }
        for &c in &a[1..] {
            match c {
                b's' => set = true,
                b'u' => unset = true,
                b'p' => print = true,
                b'q' => quiet = true,
                b'o' => o = true,
                _ => {
                    vp.error(&format!("shopt: -{}: invalid option", c as char))
                        .await;
                    vp.error_raw(format!("{USAGE}\n").as_bytes()).await;
                    return Ok(2);
                }
            }
        }
        i += 1;
    }
    if set && unset {
        vp.error("shopt: cannot set and unset shell options simultaneously")
            .await;
        return Ok(1);
    }
    let names = &args[i..];

    // Changing options.
    if (set || unset) && !names.is_empty() {
        let mut status = 0;
        for name in names {
            if o {
                match options::by_name(name) {
                    Some(opt) => super::set::set_opt(vp, opt, set),
                    None => {
                        vp.error(&format!("shopt: {}: invalid shell option name", s(name)))
                            .await;
                        status = 1;
                    }
                }
            } else {
                match std::str::from_utf8(name)
                    .ok()
                    .and_then(options::shopt_index)
                {
                    Some(idx) => {
                        vp.opts.set_shopt_at(idx, set);
                        options::sync(vp);
                    }
                    None => {
                        vp.error(&format!("shopt: {}: invalid shell option name", s(name)))
                            .await;
                        status = 1;
                    }
                }
            }
        }
        return Ok(status);
    }

    // Querying or listing. Without names: all options, or with -s/-u those on/off.
    let all: Vec<(String, bool)> = if o {
        SET_OPTS
            .iter()
            .map(|(opt, n, ..)| (n.to_string(), vp.opts.get(*opt)))
            .collect()
    } else {
        SHOPTS
            .iter()
            .enumerate()
            .map(|(idx, (n, _))| (n.to_string(), vp.opts.shopt_at(idx)))
            .collect()
    };
    let mut status = 0;
    let mut chosen: Vec<(String, bool)> = Vec::new();
    if names.is_empty() {
        chosen = all
            .into_iter()
            .filter(|(_, on)| !(set && !on || unset && *on))
            .collect();
    } else {
        for name in names {
            match all.iter().find(|(n, _)| n.as_bytes() == name.as_slice()) {
                Some((n, on)) => {
                    if !on {
                        status = 1;
                    }
                    chosen.push((n.clone(), *on));
                }
                None => {
                    vp.error(&format!("shopt: {}: invalid shell option name", s(name)))
                        .await;
                    status = 1;
                }
            }
        }
    }
    if quiet {
        return Ok(status);
    }
    let mut buf = Vec::new();
    for (n, on) in &chosen {
        let line = if print && o {
            format!("set {}o {n}\n", if *on { '-' } else { '+' })
        } else if print {
            format!("shopt {} {n}\n", if *on { "-s" } else { "-u" })
        } else if o && names.is_empty() {
            format!("{n:<15}\t{}\n", if *on { "on" } else { "off" })
        } else {
            format!("{n:<20}\t{}\n", if *on { "on" } else { "off" })
        };
        buf.extend_from_slice(line.as_bytes());
    }
    let st = out(vp, "shopt", &buf).await?;
    Ok(if st != 0 { st } else { status })
}
