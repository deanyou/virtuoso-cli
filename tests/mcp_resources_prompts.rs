//! End-to-end MCP protocol coverage for the resources/prompts surface.
//!
//! Drives the real `vcli mcp serve` stdio server against a throwaway skills
//! directory, so discovery, path safety and the prompt payloads are exercised
//! exactly as an MCP client would exercise them.

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

/// A running `vcli mcp serve` with its stdin/stdout piped.
struct Server {
    child: Child,
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl Server {
    fn start(dir: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_vcli"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", dir)
            .env("VB_HOME", dir)
            .current_dir(dir)
            .arg("mcp")
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn vcli mcp serve");

        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    /// Send one JSON-RPC request and read its response.
    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        writeln!(self.stdin, "{payload}").expect("write request");
        self.stdin.flush().expect("flush request");
        self.read_response(id)
    }

    fn read_response(&mut self, id: u64) -> Value {
        loop {
            let mut line = String::new();
            let read = self.stdout.read_line(&mut line).expect("read response");
            assert!(read > 0, "server closed stdout before answering id {id}");
            let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
                // The CLI prints its own JSON result after the loop ends.
                continue;
            };
            if value.get("id").and_then(|v| v.as_u64()) == Some(id) {
                return value;
            }
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write_skill(root: &std::path::Path, name: &str, body: &str) {
    let dir = root.join(".claude").join("skills").join(name);
    std::fs::create_dir_all(&dir).expect("create skill dir");
    std::fs::write(dir.join("SKILL.md"), body).expect("write SKILL.md");
}

fn skills_root(dir: &std::path::Path) -> std::path::PathBuf {
    dir.join(".claude").join("skills")
}

#[test]
fn initialize_declares_resources_and_prompts_capabilities() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_skill(
        tmp.path(),
        "demo",
        "---\nname: demo\ndescription: Demo skill\n---\nbody\n",
    );

    let mut server = Server::start(tmp.path());
    let response = server.request(1, "initialize", serde_json::json!({}));

    let capabilities = &response["result"]["capabilities"];
    assert!(
        capabilities.get("resources").is_some(),
        "initialize must declare the resources capability: {response}"
    );
    assert!(
        capabilities.get("prompts").is_some(),
        "initialize must declare the prompts capability: {response}"
    );
    assert!(
        capabilities.get("tools").is_some(),
        "tools capability must stay"
    );
    // The serverInfo version line is owned by another change — do not touch it.
    assert_eq!(response["result"]["serverInfo"]["name"], "virtuoso-cli");
}

#[test]
fn resources_list_and_read_expose_the_repository_skills() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let demo_body = "---\nname: demo\ndescription: Demo skill\n---\n\n# Demo\n\nfull body\n";
    write_skill(tmp.path(), "demo", demo_body);
    write_skill(
        tmp.path(),
        "other",
        "---\nname: other\ndescription: |\n  Other skill.\n  Second line.\n---\nbody\n",
    );

    let mut server = Server::start(tmp.path());
    let listed = server.request(1, "resources/list", serde_json::json!({}));
    let resources = listed["result"]["resources"]
        .as_array()
        .expect("resources array");
    assert_eq!(resources.len(), 2, "both skills must be listed: {listed}");

    let demo = resources
        .iter()
        .find(|r| r["name"] == "demo")
        .expect("demo resource");
    assert_eq!(demo["uri"], "skill://demo");
    assert_eq!(demo["mimeType"], "text/markdown");
    assert_eq!(demo["description"], "Demo skill");

    let other = resources
        .iter()
        .find(|r| r["name"] == "other")
        .expect("other resource");
    assert_eq!(other["description"], "Other skill. Second line.");

    let read = server.request(
        2,
        "resources/read",
        serde_json::json!({ "uri": "skill://demo" }),
    );
    let contents = read["result"]["contents"]
        .as_array()
        .expect("contents array");
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["uri"], "skill://demo");
    assert_eq!(contents[0]["mimeType"], "text/markdown");
    assert_eq!(
        contents[0]["text"].as_str().unwrap(),
        demo_body,
        "resources/read must return the SKILL.md text verbatim"
    );
}

#[test]
fn resources_read_rejects_traversal_and_unknown_skills() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_skill(tmp.path(), "demo", "---\nname: demo\n---\nbody\n");
    std::fs::write(tmp.path().join("secret.md"), "top secret").unwrap();

    let mut server = Server::start(tmp.path());

    for uri in [
        "skill://../secret.md",
        "skill://../../etc/passwd",
        "skill://demo/../../secret.md",
        "skill://sub/demo",
        "file:///etc/passwd",
    ] {
        let response = server.request(2, "resources/read", serde_json::json!({ "uri": uri }));
        assert!(
            !response["error"].is_null(),
            "uri '{uri}' must be rejected: {response}"
        );
        assert!(
            response["result"].is_null(),
            "uri '{uri}' must not return a result: {response}"
        );
    }

    let unknown = server.request(
        3,
        "resources/read",
        serde_json::json!({ "uri": "skill://does-not-exist" }),
    );
    assert!(
        !unknown["error"].is_null(),
        "unknown skill must be an error: {unknown}"
    );
}

#[test]
fn resources_list_is_empty_when_no_skills_directory_exists() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let mut server = Server::start(tmp.path());
    let listed = server.request(1, "resources/list", serde_json::json!({}));
    let resources = listed["result"]["resources"]
        .as_array()
        .expect("resources array");
    assert!(
        resources.is_empty(),
        "no skills dir means no resources: {listed}"
    );
    assert!(listed["error"].is_null());
}

#[test]
fn prompts_list_publishes_exactly_three_workflow_prompts() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let mut server = Server::start(tmp.path());
    let listed = server.request(1, "prompts/list", serde_json::json!({}));
    let prompts = listed["result"]["prompts"]
        .as_array()
        .expect("prompts array");
    assert_eq!(prompts.len(), 3, "exactly three prompts: {listed}");

    let names: Vec<&str> = prompts
        .iter()
        .map(|p| p["name"].as_str().expect("prompt name"))
        .collect();
    assert_eq!(
        names,
        vec![
            "maestro-run-and-check",
            "spectre-failure-triage",
            "cell-design-review"
        ]
    );
    for prompt in prompts {
        assert!(prompt["description"].is_string());
        assert!(prompt["arguments"].is_array());
    }
}

#[test]
fn prompts_get_fills_arguments_and_rejects_unknown_names() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let mut server = Server::start(tmp.path());
    let response = server.request(
        1,
        "prompts/get",
        serde_json::json!({
            "name": "maestro-run-and-check",
            "arguments": {
                "session": "fnxSession0",
                "variable": "wn",
                "value": "2u"
            }
        }),
    );

    let messages = response["result"]["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"]["type"], "text");

    let text = messages[0]["content"]["text"].as_str().expect("text");
    assert!(
        text.contains("fnxSession0"),
        "session must be filled in: {text}"
    );
    assert!(text.contains("wn"), "variable must be filled in: {text}");
    assert!(text.contains("2u"), "value must be filled in: {text}");
    // The workflow must reference the real tool names.
    assert!(text.contains("maestro_run"), "{text}");
    assert!(text.contains("maestro_get_output_value"), "{text}");

    let unknown = server.request(
        2,
        "prompts/get",
        serde_json::json!({ "name": "no-such-prompt", "arguments": {} }),
    );
    assert!(
        !unknown["error"].is_null(),
        "unknown prompt must be an error: {unknown}"
    );
}

#[test]
fn skills_are_discovered_from_the_working_directory_tree() {
    // The skills root is found by walking up from the working directory.
    let tmp = tempfile::tempdir().expect("tempdir");
    write_skill(tmp.path(), "demo", "---\nname: demo\n---\nbody\n");
    let nested = tmp.path().join("a").join("b");
    std::fs::create_dir_all(&nested).expect("nested dir");

    let mut child = Command::new(env!("CARGO_BIN_EXE_vcli"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", tmp.path())
        .env("VB_HOME", tmp.path())
        .current_dir(&nested)
        .arg("mcp")
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn vcli mcp serve");

    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    writeln!(
        stdin,
        "{}",
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "resources/list", "params": {}})
    )
    .expect("write request");
    stdin.flush().expect("flush");

    let mut line = String::new();
    stdout.read_line(&mut line).expect("read response");
    let response: Value = serde_json::from_str(line.trim()).expect("json response");
    let resources = response["result"]["resources"]
        .as_array()
        .expect("resources");
    assert_eq!(
        resources.len(),
        1,
        "the skill must be discovered from the ancestor directory: {response}"
    );
    assert_eq!(resources[0]["uri"], "skill://demo");

    let _ = child.kill();
    let _ = child.wait();
    let _ = skills_root(tmp.path());
}
