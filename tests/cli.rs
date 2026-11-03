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

#[test]
fn nested_request_fields_reject_typos_before_execution() {
    use fah::presentation::cli::commands::parse_line;
    assert!(parse_line(r#"{"op":"submit","request":{"device":{"session":1,"generation":1,"slot":1},"endpoint":0,"transfer_type":"control","direction":"in","length":0,"setup":{"request_type":128,"request":0,"value":0,"index":0,"length":0,"lenght":99}}}"#).is_err());
}

#[cfg(feature = "hosted-direct")]
#[test]
fn native_sigint_closes_idle_shell_with_valid_json() {
    use std::{
        io::{BufRead, BufReader, Write},
        process::Stdio,
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_fah"))
        .args(["shell", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(b"state\n").unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&line).is_ok());
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    line.clear();
    reader.read_line(&mut line).unwrap();
    let close: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(close["session_state"], "closed");
    assert_eq!(close["outcome"]["payload"]["reason"], "sigint");
    assert!(child.wait().unwrap().success());
}

#[cfg(feature = "hosted-direct")]
#[test]
fn independent_cli_processes_never_start_in_the_same_fixed_session() {
    use std::{io::Write, process::Stdio};
    let mut identities = Vec::new();
    for _ in 0..2 {
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
            .write_all(b"state\nquit\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let text = String::from_utf8(output.stdout).unwrap();
        let frame: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        identities.push(frame["outcome"]["payload"]["session_id"].as_u64().unwrap());
    }
    assert_ne!(identities[0], identities[1]);
}

#[test]
fn capability_report_is_available_without_hosted_backend_or_acquisition() {
    let result = Command::new(env!("CARGO_BIN_EXE_fah"))
        .arg("capabilities")
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(result.stderr.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["physical_validated"], false);
    assert!(report["capabilities"].as_array().unwrap().len() > 30);
}
