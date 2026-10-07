use std::process::Command;

fn prr() -> Command {
    Command::new(env!("CARGO_BIN_EXE_prr"))
}

#[test]
fn version_prints() {
    let out = prr().arg("--version").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("prr "));
}

#[test]
fn schema_describes_the_findings_file() {
    let out = prr().arg("schema").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let schema: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(schema["properties"]["findings"].is_object(), "{schema}");
}
