//! The client's side: what a later `fenix` launch uses to hand its
//! files to the running one, and the reference for other tools' own
//! clients.

use std::fmt;
use std::io::{self, BufReader, Read, Write};

use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::endpoint::{Endpoint, Stream};
use crate::protocol::{
    method, ClientInfo, HelloParams, HelloResult, OpenParams, OpenProjectParams, RpcError,
    StateResult, MAX_BODY, MAX_HEADER_LINE, PROTOCOL,
};

/// Why a request didn't get a result.
#[derive(Debug)]
pub enum ClientError {
    /// The connection failed or closed early.
    Io(io::Error),
    /// Fenix answered with an error.
    Rpc(RpcError),
    /// Fenix answered with something that isn't a reply to the request.
    Protocol(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Io(e) => write!(f, "{e}"),
            ClientError::Rpc(e) => write!(f, "{e}"),
            ClientError::Protocol(e) => write!(f, "unexpected reply: {e}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<io::Error> for ClientError {
    fn from(e: io::Error) -> Self {
        ClientError::Io(e)
    }
}

/// A connection to a running Fenix, one request at a time.
pub struct Client<R, W> {
    reader: BufReader<R>,
    writer: W,
    next_id: u64,
}

impl Client<Stream, Stream> {
    /// Connects to the Fenix serving `endpoint`; `None` when none is.
    pub fn connect(endpoint: &Endpoint) -> io::Result<Option<Self>> {
        let Some(stream) = endpoint.connect()? else {
            return Ok(None);
        };
        let writer = stream.try_clone()?;
        Ok(Some(Client::new(stream, writer)))
    }
}

impl<R: Read, W: Write> Client<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Client {
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
        }
    }

    /// Sends one request and waits for its reply.
    pub fn request<T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<T, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        fenix_rpc::write_message(
            &mut self.writer,
            &serde_json::to_vec(&request).expect("plain data serializes"),
        )?;
        let body = fenix_rpc::read_message_limited(&mut self.reader, MAX_BODY, MAX_HEADER_LINE)?
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "Fenix closed the connection")
            })?;
        let reply: Value =
            serde_json::from_slice(&body).map_err(|e| ClientError::Protocol(e.to_string()))?;
        if reply.get("id") != Some(&json!(id)) {
            return Err(ClientError::Protocol(format!(
                "a reply for another request: {reply}"
            )));
        }
        if let Some(error) = reply.get("error") {
            let error = serde_json::from_value(error.clone())
                .map_err(|e| ClientError::Protocol(e.to_string()))?;
            return Err(ClientError::Rpc(error));
        }
        let result = reply
            .get("result")
            .cloned()
            .ok_or_else(|| ClientError::Protocol(format!("no result: {reply}")))?;
        serde_json::from_value(result).map_err(|e| ClientError::Protocol(e.to_string()))
    }

    /// `fenix/hello`, introducing this client as `name` `version`.
    pub fn hello(&mut self, name: &str, version: &str) -> Result<HelloResult, ClientError> {
        let params = HelloParams {
            protocol: PROTOCOL,
            client: ClientInfo {
                name: name.to_string(),
                version: version.to_string(),
                pid: std::process::id(),
            },
        };
        self.request(
            method::HELLO,
            serde_json::to_value(params).expect("plain data serializes"),
        )
    }

    pub fn open(&mut self, params: &OpenParams) -> Result<(), ClientError> {
        self.request::<Value>(
            method::OPEN,
            serde_json::to_value(params).expect("plain data serializes"),
        )
        .map(drop)
    }

    pub fn open_project(&mut self, root: &str) -> Result<(), ClientError> {
        let params = OpenProjectParams {
            root: root.to_string(),
        };
        self.request::<Value>(
            method::OPEN_PROJECT,
            serde_json::to_value(params).expect("plain data serializes"),
        )
        .map(drop)
    }

    pub fn state(&mut self) -> Result<StateResult, ClientError> {
        self.request(method::STATE, json!({}))
    }
}
