//! Exercise the shipped executable and its local API from a separate client process.
// RevExt: 1
use std::{
    net::TcpListener,
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};
// RevExt: 2
use anyhow::{Result, ensure};
use serde_json::Value;
// RevExt: 3
const BIN: &str = env!("CARGO_BIN_EXE_provider-router");
// RevExt: 4
struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }  // RevExt: 21
}  // RevExt: 23
// RevExt: 5
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
    );  // RevExt: 31
    Ok(serde_json::from_slice(&output.stdout)?)
}  // RevExt: 24
// RevExt: 6
#[test]  // RevExt: 40
fn rejects_invalid_and_unassigned_bridge_hosts() -> Result<()> {
    let temp = tempfile::tempdir()?;  // RevExt: 42
    let profile = temp.path().join("profile");  // RevExt: 46
    let cases = [
        ("localhost", "invalid IP address"),
        ("0.0.0.0", "must be an RFC 1918"),
        ("8.8.8.8", "must be an RFC 1918"),
        ("127.0.0.1", "must be an RFC 1918"),
        ("fe80::1", "must be an RFC 1918"),
        (
            "fd4d:530d:76d4:2fb9::1407",
            if cfg!(target_os = "linux") {
                "not assigned to a local network interface"
            } else {
                "supported on Linux only"
            },
        ),
    ];
    for (host, message) in cases {
        let output = Command::new(BIN)
            .arg("--data-dir")  // RevExt: 50
            .arg(&profile)  // RevExt: 53
            .args(["network", "set", "--container-bridge-host", host])
            .output()?;
        assert!(!output.status.success(), "{host}");
        let error = String::from_utf8(output.stderr)?;
        assert!(error.contains(message), "{error}");
    }  // RevExt: 22
    assert!(cli(&profile, &["network", "show"])?["container_bridge_host"].is_null());
    Ok(())  // RevExt: 56
}  // RevExt: 25
// RevExt: 7
#[cfg(target_os = "linux")]  // RevExt: 61
#[tokio::test]  // RevExt: 64
async fn adds_and_removes_bridge_access_without_restarting_loopback() -> Result<()> {
    // Run this integration scenario where the Linux Docker bridge is visible.
    let Some(bridge_ip) = std::env::var_os("PROVIDER_ROUTER_TEST_DOCKER_BRIDGE_IP") else {
        return Ok(());
    };
    let bridge_ip: std::net::Ipv4Addr = bridge_ip.to_string_lossy().parse()?;
    let conflict = TcpListener::bind((bridge_ip, 0))?;
    let port = conflict.local_addr()?.port();
    let temp = tempfile::tempdir()?;  // RevExt: 43
    let profile = temp.path().join("profile");  // RevExt: 47
    cli(&profile, &["network", "set", "--port", &port.to_string()])?;
    let mut server = Running(  // RevExt: 66
        Command::new(BIN)  // RevExt: 68
            .arg("--data-dir")  // RevExt: 51
            .arg(&profile)  // RevExt: 54
            .arg("serve")  // RevExt: 70
            .stdin(Stdio::null())  // RevExt: 72
            .stdout(Stdio::null())  // RevExt: 74
            .stderr(Stdio::inherit())  // RevExt: 76
            .spawn()?,  // RevExt: 78
    );  // RevExt: 32
    let client = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(1)).build()?;
    let loopback = format!("http://127.0.0.1:{port}");
    let bridge = format!("http://{bridge_ip}:{port}");
    let alias = format!("host.docker.internal:{port}");
    let first = wait_for_status(&profile, |status| status["endpoint"] == loopback).await?;
    let loopback_health = format!("{loopback}/health");
    assert_eq!(  // RevExt: 80
        client.get(&loopback_health).header("Host", &alias).send().await?.status(),  // RevExt: 86
        403  // RevExt: 88
    );  // RevExt: 33
// RevExt: 8
    let bridge_host = bridge_ip.to_string();
    cli(&profile, &["network", "set", "--container-bridge-host", &bridge_host])?;
    assert_eq!(cli(&profile, &["network", "show"])?["container_bridge_host"], bridge_host);
    let unavailable = wait_for_status(&profile, |status| status["error"].is_string()).await?;
    assert!(unavailable["error"].as_str().unwrap().contains("Docker bridge listener unavailable"));
    assert!(unavailable["container_bridge_endpoint"].is_null());
    assert_eq!(client.get(&loopback_health).send().await?.status(), 200);  // RevExt: 91
// RevExt: 9
    drop(conflict);
    let running =
        wait_for_status(&profile, |status| status["container_bridge_endpoint"] == bridge).await?;
    assert_eq!(running["endpoint"], loopback);
    assert!(running["error"].is_null());
    let bridge_health = format!("{bridge}/health");
    let ip_health = client.get(&bridge_health).send().await?.error_for_status()?.json::<Value>().await?;
    let local_health = client.get(&loopback_health).send().await?.error_for_status()?.json::<Value>().await?;
    assert_eq!(ip_health, local_health);
    assert_eq!(ip_health["instance_id"], first["instance_id"]);
    assert_eq!(ip_health["authentication_required"], false);
    let models = format!("{bridge}/v1/models");
    assert_eq!(client.get(&models).header("Host", &alias).send().await?.status(), 200);
    assert_eq!(client.get(&models).header("Host", "unrelated.example").send().await?.status(), 403);
    let rejected = client.get(&models).header("Origin", &bridge).send().await?;
    assert_eq!(rejected.status(), 403);
    assert_eq!(rejected.json::<Value>().await?["error"]["code"], "BROWSER_ORIGIN_REJECTED");
// RevExt: 10
    cli(&profile, &["network", "set", "--clear-container-bridge-host"])?;
    let disabled =
        wait_for_status(&profile, |status| status["container_bridge_endpoint"].is_null()).await?;
    assert_eq!(disabled["endpoint"], loopback);
    assert!(disabled["network"]["container_bridge_host"].is_null());
    assert!(client.get(&bridge_health).send().await.is_err());
    assert_eq!(client.get(&loopback_health).send().await?.status(), 200);  // RevExt: 92
    assert_eq!(  // RevExt: 81
        client.get(&loopback_health).header("Host", &alias).send().await?.status(),  // RevExt: 87
        403  // RevExt: 89
    );  // RevExt: 34
    cli(&profile, &["stop"])?;  // RevExt: 93
    assert!(server.0.wait()?.success());  // RevExt: 95
    Ok(())  // RevExt: 57
}  // RevExt: 26
// RevExt: 11
#[cfg(target_os = "linux")]  // RevExt: 62
async fn wait_for_status(profile: &Path, ready: impl Fn(&Value) -> bool) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {  // RevExt: 97
            let status = cli(profile, &["status"])?;
            if ready(&status) {
                return Ok(status);
            }  // RevExt: 100
            tokio::time::sleep(Duration::from_millis(60)).await;  // RevExt: 103
        }  // RevExt: 106
    })  // RevExt: 109
    .await?
}  // RevExt: 27
// RevExt: 12
#[tokio::test]  // RevExt: 65
async fn applies_shared_client_and_network_settings_to_the_running_api() -> Result<()> {
    let temp = tempfile::tempdir()?;  // RevExt: 44
    let profile = temp.path().join("profile");  // RevExt: 48
    let reservation = TcpListener::bind(("127.0.0.1", 0))?;
    let port = reservation.local_addr()?.port();
    drop(reservation);
// RevExt: 13
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
    let mut server = Running(  // RevExt: 67
        Command::new(BIN)  // RevExt: 69
            .arg("--data-dir")  // RevExt: 52
            .arg(&profile)  // RevExt: 55
            .arg("serve")  // RevExt: 71
            .stdin(Stdio::null())  // RevExt: 73
            .stdout(Stdio::null())  // RevExt: 75
            .stderr(Stdio::inherit())  // RevExt: 77
            .spawn()?,  // RevExt: 79
    );  // RevExt: 35
    let endpoint = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(1))
        .build()?;
    let health_url = format!("{endpoint}/health");
    let health = tokio::time::timeout(Duration::from_secs(10), async {
        loop {  // RevExt: 98
            if let Ok(response) = client.get(&health_url).send().await
                && response.status().is_success()
            {
                break response.json::<Value>().await;
            }  // RevExt: 101
            tokio::time::sleep(Duration::from_millis(60)).await;  // RevExt: 104
        }  // RevExt: 107
    })  // RevExt: 110
    .await??;
    assert_eq!(health["instance_id"], idle["instance_id"]);
    assert_eq!(health["authentication_required"], true);
// RevExt: 14
    let models = format!("{endpoint}/v1/models");
    assert_eq!(client.get(&models).send().await?.status(), 401);
    let grant = cli(&profile, &["clients", "create", "SDK client"])?;
    let token = grant["token"].as_str().unwrap();
    let id = grant["id"].as_str().unwrap();
    let active = client.get(&models).bearer_auth(token).send().await?;
    assert_eq!(active.status(), 200);
    assert_eq!(active.json::<Value>().await?["data"], serde_json::json!([]));
// RevExt: 15
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
    assert_eq!(  // RevExt: 82
        check["choices"][0]["message"]["content"],
        "Attachments fit the request."
    );  // RevExt: 36
    assert_eq!(check["usage"]["total_tokens"], 0);
// RevExt: 16
    let clients = cli(&profile, &["clients", "list"])?;
    assert_eq!(clients[0]["name"], "SDK client");
    assert!(clients[0].get("token_hash").is_none());
    cli(&profile, &["clients", "revoke", id])?;
    assert_eq!(  // RevExt: 83
        client
            .get(&models)
            .bearer_auth(token)
            .send()
            .await?
            .status(),
        401
    );  // RevExt: 37
// RevExt: 17
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
        loop {  // RevExt: 99
            if let Ok(response) = client
                .get(&new_models)
                .header("Origin", "https://example.com")
                .send()
                .await
            {
                break response;
            }  // RevExt: 102
            tokio::time::sleep(Duration::from_millis(60)).await;  // RevExt: 105
        }  // RevExt: 108
    })  // RevExt: 111
    .await?;
    assert_eq!(origin.status(), 200);
    assert_eq!(  // RevExt: 84
        origin.headers()["access-control-allow-origin"],
        "https://example.com"
    );  // RevExt: 38
    assert_eq!(  // RevExt: 85
        client
            .get(&new_models)
            .header("Origin", "https://other.example.com")
            .send()
            .await?
            .status(),
        403  // RevExt: 90
    );  // RevExt: 39
    cli(&profile, &["stop"])?;  // RevExt: 94
    assert!(server.0.wait()?.success());  // RevExt: 96
    assert!(cli(&profile, &["status"])?["endpoint"].is_null());
    Ok(())  // RevExt: 58
}  // RevExt: 28
// RevExt: 18
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
    Ok(())  // RevExt: 59
}  // RevExt: 29
// RevExt: 19
#[cfg(target_os = "linux")]  // RevExt: 63
#[test]  // RevExt: 41
fn stores_openrouter_credentials_without_printing_or_persisting_plaintext() -> Result<()> {
    use std::io::Write;
// RevExt: 20
    let temp = tempfile::tempdir()?;  // RevExt: 45
    let profile = temp.path().join("profile");  // RevExt: 49
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
    Ok(())  // RevExt: 60
}  // RevExt: 30
