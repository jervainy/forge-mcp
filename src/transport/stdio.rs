use anyhow::Context;
use serde_json::Value;
use tokio::io::{
    AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter,
};

use crate::{
    protocol::jsonrpc::{JsonRpcMessage, JsonRpcResponse},
    server::ForgeServer,
};

pub async fn serve(mut server: ForgeServer) -> anyhow::Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let mut lines = BufReader::new(stdin).lines();
    let mut writer = BufWriter::new(stdout);

    while let Some(line) = lines.next_line().await? {
        let response = match serde_json::from_str::<Value>(&line) {
            Err(_) => Some(JsonRpcResponse::parse_error()),
            Ok(value) => match serde_json::from_value::<JsonRpcMessage>(value) {
                Err(error) => Some(JsonRpcResponse::invalid_request(error.to_string())),
                Ok(message) => server.handle(message).await,
            },
        };

        if let Some(response) = response {
            let encoded = serde_json::to_vec(&response)
                .context("failed to serialize JSON-RPC response")?;
            writer.write_all(&encoded).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
    }

    Ok(())
}
