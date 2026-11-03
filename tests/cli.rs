use std::process::Command;

#[test]
fn help_and_version_work_without_devices() {
    for args in [vec![], vec!["--help"], vec!["--version"]] {
        let result = Command::new(env!("CARGO_BIN_EXE_fah"))
            .args(args)
            .output()
            .unwrap();
        assert!(result.status.success());
        assert!(!result.stdout.is_empty());
        assert!(result.stderr.is_empty());
    }
}

#[test]
fn invalid_arguments_have_stable_exit_status() {
    for args in [vec!["acquire"], vec!["--help", "extra"]] {
        let result = Command::new(env!("CARGO_BIN_EXE_fah"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(result.stdout.is_empty());
        assert!(!result.stderr.is_empty());
    }
}

#[test]
fn input_limit_drains_one_line_and_preserves_next_command() {
    use fah::presentation::cli::{commands::MAX_LINE, runtime::read_command};
    let mut input = vec![b'x'; MAX_LINE + 1];
    input.extend_from_slice(b"\nstate\n");
    let mut reader = std::io::Cursor::new(input);
    assert!(read_command(&mut reader).unwrap().unwrap().is_err());
    assert_eq!(
        read_command(&mut reader).unwrap().unwrap().unwrap(),
        "state\n"
    );
    assert!(read_command(&mut reader).unwrap().is_none());
}
#[cfg(feature = "hosted-direct")]
#[test]
fn subprocess_shell_help_and_eof_are_json_without_acquisition() {
    use std::{io::Write, process::Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_fah"))
        .args(["shell", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"help\nstate\ncapabilities\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let frames: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(frames.len(), 4);
    assert!(
        frames
            .iter()
            .all(|frame| frame["session_state"] == "closed")
    );
}
