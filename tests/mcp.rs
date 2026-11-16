//! `poo mcp` over a real stdio session: the handshake, the tool list agents see, and
//! calls that need no scan. Runs against an empty home.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

struct Session {
    child: std::process::Child,
    out: BufReader<std::process::ChildStdout>,
    next: u64,
}

impl Session {
    fn start() -> Session {
        let home = std::env::temp_dir().join(format!("poo-mcp-test-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_poo"))
            .arg("mcp")
            .env("HOME", &home)
            .env("POO_HOME", home.join(".poo"))
            .env("POO_NO_UPDATE_CHECK", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        Session { child, out, next: 0 }
    }

    fn send(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params});
        writeln!(self.child.stdin.as_mut().unwrap(), "{}", msg).unwrap();
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], json!(self.next), "response id");
        v
    }

    fn notify(&mut self, method: &str) {
        writeln!(self.child.stdin.as_mut().unwrap(), "{}", json!({"jsonrpc": "2.0", "method": method})).unwrap();
    }

    fn call(&mut self, name: &str, args: Value) -> Value {
        self.send("tools/call", json!({"name": name, "arguments": args}))["result"].clone()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn handshake_tools_and_cheap_calls() {
    let mut s = Session::start();
    let init = s.send("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}));
    assert_eq!(init["result"]["serverInfo"]["name"], "poo");
    assert!(init["result"]["instructions"].as_str().unwrap().contains("overview"));
    s.notify("notifications/initialized");

    let tools = s.send("tools/list", json!({}))["result"]["tools"].as_array().unwrap().clone();
    assert!(tools.len() >= 10);
    let listed = serde_json::to_string(&tools).unwrap();
    assert!(listed.len() < 16_000, "tools/list grew to {} bytes; every byte is in each agent's context", listed.len());
    for t in &tools {
        let name = t["name"].as_str().unwrap();
        let desc = t["description"].as_str().unwrap_or("");
        assert!(desc.len() >= 30 && desc.len() <= 600, "{}: description of {} chars", name, desc.len());
        assert_eq!(t["inputSchema"]["type"], "object", "{}", name);
        let ann = &t["annotations"];
        assert!(ann["readOnlyHint"].as_bool().unwrap_or(false) || ann["destructiveHint"].as_bool().unwrap_or(false), "{} says neither read-only nor destructive", name);
    }

    let h = s.call("history", json!({}));
    assert_eq!(h["isError"], false);
    assert_eq!(h["structuredContent"]["history"], json!([]));

    let c = s.call("catalog", json!({}));
    assert_eq!(c["isError"], false);
    let text = c["content"][0]["text"].as_str().unwrap();
    assert!(text.len() < 24_000, "catalog summary is {} bytes", text.len());

    let bad = s.call("no_such_tool", json!({}));
    assert_eq!(bad["isError"], true);
    let msg = bad["structuredContent"]["error"]["message"].as_str().unwrap();
    assert!(msg.contains("list_items"), "unknown tool error should list the tools: {}", msg);

    assert_eq!(s.send("ping", json!({}))["result"], json!({}));
}
