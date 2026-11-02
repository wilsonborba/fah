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
