//! Exercise the shipped executable and its local API from a separate client process.

use std::{
    net::TcpListener,
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};

use anyhow::{Result, ensure};
use serde_json::Value;

const BIN: &str = env!("CARGO_BIN_EXE_provider-router");

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn cli(profile: &Path, args: &[&str]) -> Result<Value> {
    let output = Command::new(BIN)
        .arg("--data-dir")
        .arg(profile)
        .args(args)
        .output()?;
    ensure!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[tokio::test]
async fn applies_shared_client_and_network_settings_to_the_running_api() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let profile = temp.path().join("profile");
    let reservation = TcpListener::bind(("127.0.0.1", 0))?;
    let port = reservation.local_addr()?.port();
    drop(reservation);

    let idle = cli(&profile, &["status"])?;
    assert_eq!(idle["startup_mode"], "startup");
    assert!(idle["endpoint"].is_null());
    cli(
        &profile,
        &[
            "network",
            "set",
            "--port",
            &port.to_string(),
            "--require-key",
        ],
    )?;
    let mut server = Running(
        Command::new(BIN)
            .arg("--data-dir")
            .arg(&profile)
            .arg("serve")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()?,
    );
    let endpoint = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(1))
        .build()?;
    let health_url = format!("{endpoint}/health");
    let health = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(response) = client.get(&health_url).send().await
                && response.status().is_success()
            {
                break response.json::<Value>().await;
            }
            tokio::time::sleep(Duration::from_millis(60)).await;
        }
    })
    .await??;
    assert_eq!(health["instance_id"], idle["instance_id"]);
    assert_eq!(health["authentication_required"], true);

    let models = format!("{endpoint}/v1/models");
    assert_eq!(client.get(&models).send().await?.status(), 401);
    let grant = cli(&profile, &["clients", "create", "SDK client"])?;
    let token = grant["token"].as_str().unwrap();
    let id = grant["id"].as_str().unwrap();
    let active = client.get(&models).bearer_auth(token).send().await?;
    assert_eq!(active.status(), 200);
    assert_eq!(active.json::<Value>().await?["data"], serde_json::json!([]));

    let provider = cli(&profile, &["providers", "add", "codex"])?;
    let mut connection = provider.clone();
    connection["exposed"] = true.into();
    connection["models"] = serde_json::json!([{
        "id":"synthetic", "name":"Synthetic text model", "reasoning_efforts":[],
        "default_reasoning_effort":null, "context_window":64000, "max_output_tokens":2000,
        "tools":false, "structured_output":false, "input_modalities":["text"]
    }]);
    apply(
        &profile,
        serde_json::json!({"type":"save_connection", "connection":connection}),
    )?;
    let attachment = client.post(format!("{endpoint}/v1/attachment-checks"))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "model":format!("{}/synthetic", provider["id"].as_str().unwrap()),
            "messages":[{"role":"user","content":[{
                "type":"file","file":{"filename":"note.txt","file_data":"data:text/plain;base64,SGVsbG8h"}
            }]}]
        })).send().await?;
    assert_eq!(attachment.status(), 200);
    let check: Value = attachment.json().await?;
    assert_eq!(
        check["choices"][0]["message"]["content"],
        "Attachments fit the request."
    );
    assert_eq!(check["usage"]["total_tokens"], 0);

    let clients = cli(&profile, &["clients", "list"])?;
    assert_eq!(clients[0]["name"], "SDK client");
    assert!(clients[0].get("token_hash").is_none());
    cli(&profile, &["clients", "revoke", id])?;
    assert_eq!(
        client
            .get(&models)
            .bearer_auth(token)
            .send()
            .await?
            .status(),
        401
    );

    let new_reservation = TcpListener::bind(("127.0.0.1", 0))?;
    let new_port = new_reservation.local_addr()?.port();
    drop(new_reservation);
    cli(
        &profile,
        &[
            "network",
            "set",
            "--port",
            &new_port.to_string(),
            "--no-require-key",
            "--origin",
            "https://example.com",
        ],
    )?;
    let new_models = format!("http://127.0.0.1:{new_port}/v1/models");
    let origin = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(response) = client
                .get(&new_models)
                .header("Origin", "https://example.com")
                .send()
                .await
            {
                break response;
            }
            tokio::time::sleep(Duration::from_millis(60)).await;
        }
    })
    .await?;
    assert_eq!(origin.status(), 200);
    assert_eq!(
        origin.headers()["access-control-allow-origin"],
        "https://example.com"
    );
    assert_eq!(
        client
            .get(&new_models)
            .header("Origin", "https://other.example.com")
            .send()
            .await?
            .status(),
        403
    );
    cli(&profile, &["stop"])?;
    assert!(server.0.wait()?.success());
    assert!(cli(&profile, &["status"])?["endpoint"].is_null());
    Ok(())
}

fn apply(profile: &Path, command: Value) -> Result<()> {
    use std::io::Write;
    let mut child = Command::new(BIN)
        .arg("--data-dir")
        .arg(profile)
        .args(["settings", "apply"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&command)?)?;
    ensure!(child.wait()?.success(), "The settings command failed.");
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn stores_openrouter_credentials_without_printing_or_persisting_plaintext() -> Result<()> {
    use std::io::Write;

    let temp = tempfile::tempdir()?;
    let profile = temp.path().join("profile");
    let fake_key = "synthetic-openrouter-test-key";
    let mut child = Command::new(BIN)
        .arg("--data-dir")
        .arg(&profile)
        .args(["providers", "add", "openrouter", "--key-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    child.stdin.take().unwrap().write_all(fake_key.as_bytes())?;
    let output = child.wait_with_output()?;
    assert!(output.status.success());
    let providers = cli(&profile, &["providers", "list"])?;
    assert_eq!(providers[0]["method"], "openrouter_http");
    assert!(!String::from_utf8(output.stdout)?.contains(fake_key));
    assert!(!std::fs::read_to_string(profile.join("router.json"))?.contains(fake_key));
    Ok(())
}
