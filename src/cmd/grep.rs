use std::path::PathBuf;
use std::process::Command;

use anyhow::Result;

use crate::light_command::{set_current_dir, LightCommand};

fn prepare_grep_and_args(cmd_str: &str, cmd_dir: Option<PathBuf>) -> (Command, Vec<String>) {
    let args = cmd_str
        .split_whitespace()
        .map(Into::into)
        .collect::<Vec<String>>();

    let mut cmd = Command::new(args[0].clone());

    set_current_dir(&mut cmd, cmd_dir);

    (cmd, args)
}

fn truncate_long_matched_grep_lines(lines: Vec<String>, winwidth: usize) {
    use regex::Regex;
    lazy_static::lazy_static! {
        static ref RE: Regex = Regex::new(r"^(.*):(\d+):(\d+):").unwrap();
    }
    let line =" core/proofs/proofs.hpp:138:57:    static outcome::result<std::vector<PoStCandidateWithTicket>>";
    let m1 = RE.captures(line).and_then(|cap| cap.get(1));
}

pub fn run(
    grep_cmd: String,
    grep_query: String,
    glob: Option<String>,
    cmd_dir: Option<PathBuf>,
    number: Option<usize>,
    enable_icon: bool,
) -> Result<()> {
    let (mut cmd, mut args) = prepare_grep_and_args(&grep_cmd, cmd_dir);

    // We split out the grep opts and query in case of the possible escape issue of clap.
    args.push(grep_query.to_string());

    if let Some(g) = glob {
        args.push("-g".into());
        args.push(g);
    }

    // currently vim-clap only supports rg.
    // Ref https://github.com/liuchengxu/vim-clap/pull/60
    if cfg!(windows) {
        args.push(".".into());
    }

    cmd.args(&args[1..]);

    let mut light_cmd = LightCommand::new_grep(&mut cmd, number, enable_icon);

    if let Some((total, lines, tempfile)) = light_cmd.execute_and_gather_output(&args)? {
        let lines = truncate_long_matched_grep_lines(lines, 62);
        if let Some(tempfile) = tempfile {
            println_json!(total, lines, tempfile);
        } else {
            println_json!(total, lines);
        }
    }

    Ok(())
}

#[test]
fn grep_truncate_long_lines() {
    use regex::Regex;
    lazy_static::lazy_static! {
        static ref RE: Regex = Regex::new(r"^(.*):(\d+):(\d+)(:)").unwrap();
    }
    let line =" core/proofs/proofs.hpp:138:57:    static outcome::result<std::vector<PoStCandidateWithTicket>>";
    let m1 = RE
        .captures(line)
        .and_then(|cap| cap.get(1).map(|x| x.as_str()));
    let m2 = RE
        .captures(line)
        .and_then(|cap| cap.get(2).map(|x| x.as_str()))
        .unwrap();
    let lnum = m2.parse::<usize>().unwrap();
    let m3 = RE
        .captures(line)
        .and_then(|cap| cap.get(3).map(|x| x.as_str()));
    let col = m3.unwrap().parse::<usize>().unwrap();
    let m4 = RE.captures(line).and_then(|cap| cap.get(4));
    let start_offset = m4.map(|x| x.start()).unwrap();
    let last_offset = m4.map(|x| x.end()).unwrap();
    println!("m1: {:?}", col);
    println!("lnum: {:?}", lnum);
    println!("col: {:?}", col);
    println!("m4: {:?}", m4);
    println!("last_offset: {:?}", last_offset);

    let start_idx_in_line = start_offset + last_offset;
    let winwidth: usize = 62;
    // [----------------------]
    //                       [----------------------]
    // [----------------------------------xxxxx-----]
    let my_start = if start_offset > winwidth {
        line.len() - winwidth
    } else if start_idx_in_line + 10 > winwidth {
        start_idx_in_line + 10 - winwidth
    } else {
        0
    };
    println!(" raw_line: {}", line);
    println!(" raw_line: {}", "-".repeat(62));
    println!("truncated: {}", &line[my_start..]);
}
