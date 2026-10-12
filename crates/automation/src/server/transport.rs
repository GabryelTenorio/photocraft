//! Recover malformed JSON lines before passing valid messages to rmcp.
use super::*;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::time::{Duration, timeout};

/// Inactive MCP stdio processes must not live forever after their client disappears (#3660).
/// A live client can reconnect/restart the process; clean EOF still closes it immediately.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

impl PhotocraftMcp {
    /// Serve a JSON-lines stream. A malformed line receives -32700/id:null; the next line
    /// is still handled. A single writer serializes parse errors and SDK responses.
    pub async fn serve_io(
        self,
        input: impl tokio::io::AsyncRead + Unpin + Send + 'static,
        output: impl tokio::io::AsyncWrite + Unpin + Send + 'static,
    ) -> Result<(), AutomationError> {
        self.serve_io_with_idle(input, output, IDLE_TIMEOUT).await
    }

    async fn serve_io_with_idle(
        self,
        input: impl tokio::io::AsyncRead + Unpin + Send + 'static,
        output: impl tokio::io::AsyncWrite + Unpin + Send + 'static,
        idle: Duration,
    ) -> Result<(), AutomationError> {
        let (mut to_service, service_in) = tokio::io::duplex(1 << 20);
        let (service_out, from_service) = tokio::io::duplex(1 << 20);
        let (errors, mut error_rx) = tokio::sync::mpsc::channel::<String>(16);
        let reader = tokio::spawn(async move {
            let mut lines = BufReader::new(input).lines();
            loop {
                // EOF may never arrive if a dead client leaves a pipe open. Count time
                // between incoming requests, not time since the process launched.
                let line = match timeout(idle, lines.next_line()).await {
                    Ok(Ok(Some(line))) => line,
                    Ok(Ok(None)) | Err(_) => break,
                    Ok(Err(error)) => return Err(error),
                };
                if line.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(&line) {
                    Ok(_) => to_service.write_all(format!("{line}\n").as_bytes()).await?,
                    Err(e) => {
                        let reply = json!({"jsonrpc":"2.0", "id":null, "error":{"code":-32700, "message":format!("parse error: {e}")}});
                        if errors.send(reply.to_string()).await.is_err() {
                            break;
                        }
                    }
                }
            }
            to_service.shutdown().await
        });
        let writer = tokio::spawn(async move {
            let mut output = output;
            let mut replies = BufReader::new(from_service).lines();
            loop {
                let line = tokio::select! {
                    biased;
                    Some(error) = error_rx.recv() => error,
                    reply = replies.next_line() => match reply? { Some(line) => line, None => break },
                };
                output.write_all(format!("{line}\n").as_bytes()).await?;
                output.flush().await?;
            }
            Ok::<_, std::io::Error>(())
        });
        let result = match self.serve((service_in, service_out)).await {
            Ok(running) => running.waiting().await.map(|_| ()).map_err(|e| AutomationError::Other(e.to_string())),
            Err(e) => Err(AutomationError::Other(format!("MCP init: {e}"))),
        };
        reader.abort();
        match reader.await {
            Ok(r) => r.map_err(|e| AutomationError::Io(e.to_string()))?,
            Err(e) if !e.is_cancelled() => return Err(AutomationError::Other(e.to_string())),
            Err(_) => {}
        }
        writer.await.map_err(|e| AutomationError::Other(e.to_string()))?.map_err(|e| AutomationError::Io(e.to_string()))?;
        result
    }
}

#[cfg(test)]
mod idle_tests {
    use super::*;

    async fn initialise(client: &mut tokio::io::DuplexStream, replies: &mut tokio::io::Lines<BufReader<tokio::io::DuplexStream>>) {
        let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}
        }});
        client.write_all(format!("{init}\n").as_bytes()).await.unwrap();
        let reply = timeout(Duration::from_secs(2), replies.next_line()).await.unwrap().unwrap().unwrap();
        let reply: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["id"], 1);
        assert!(reply["result"]["serverInfo"]["name"].is_string(), "{reply}");
        client.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n").await.unwrap();
    }

    #[tokio::test]
    async fn orphaned_client_pipe_exits_after_inactivity() {
        // Keep the client end open: the server cannot depend on stdin EOF to exit.
        let (mut client, input) = tokio::io::duplex(4096);
        let (output, responses) = tokio::io::duplex(4096);
        let server = tokio::spawn(PhotocraftMcp::headless().serve_io_with_idle(input, output, Duration::from_millis(250)));
        let mut replies = BufReader::new(responses).lines();
        initialise(&mut client, &mut replies).await;
        let exit = timeout(Duration::from_secs(3), server).await;
        assert!(matches!(&exit, Ok(Ok(Ok(())))), "MCP server did not exit cleanly after inactivity: {exit:?}");
    }

    #[tokio::test]
    async fn incoming_lines_reset_the_idle_deadline() {
        let (mut client, input) = tokio::io::duplex(4096);
        let (output, responses) = tokio::io::duplex(4096);
        let server = tokio::spawn(PhotocraftMcp::headless().serve_io_with_idle(input, output, Duration::from_millis(300)));
        let mut replies = BufReader::new(responses).lines();
        initialise(&mut client, &mut replies).await;

        // Even malformed requests count as traffic and still receive a parse error.
        for _ in 0..3 {
            client.write_all(b"not-json\n").await.unwrap();
            let reply = timeout(Duration::from_secs(2), replies.next_line()).await.unwrap().unwrap().unwrap();
            let reply: Value = serde_json::from_str(&reply).unwrap();
            assert_eq!(reply["error"]["code"], -32700);
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert!(!server.is_finished(), "active transport shut down before its idle deadline");
        }
        let exit = timeout(Duration::from_secs(3), server).await;
        assert!(matches!(&exit, Ok(Ok(Ok(())))), "MCP server did not close after requests stopped: {exit:?}");
    }
}
