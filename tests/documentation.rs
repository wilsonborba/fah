#[test]
fn readme_json_lines_use_the_actual_protocol_types() {
    use fah::{domain::models::OperationResult, presentation::cli::commands::ShellCommand};
    let text = include_str!("../README.md");
    let mut count = 0;
    for block in text.split("```json\n").skip(1) {
        let sample = block.split("```").next().unwrap();
        for line in sample.lines().filter(|line| !line.trim().is_empty()) {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            if value.get("op").is_some() {
                serde_json::from_value::<ShellCommand>(value).unwrap();
            } else {
                serde_json::from_value::<OperationResult<serde_json::Value>>(value).unwrap();
            }
            count += 1;
        }
    }
    assert!(count >= 8);
}
