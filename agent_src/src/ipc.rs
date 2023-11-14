use hbb_common::{
    allow_err, bail, bytes,
    bytes_codec::BytesCodec,
    config::{self, Config},
    futures::StreamExt as _,
    futures_util::sink::SinkExt,
    timeout, tokio,
    tokio::io::{AsyncRead, AsyncWrite},
    tokio_util::codec::Framed,
    ResultType,
};
use parity_tokio_ipc::{
    Connection as Conn, ConnectionClient as ConnClient, Endpoint, Incoming, SecurityAttributes,
};
use serde_derive::{Deserialize, Serialize};
use std::{collections::HashMap, net::SocketAddr};
#[cfg(not(windows))]
use std::{fs::File, io::prelude::*};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "t", content = "c")]
pub enum FS {
    ReadDir {
        dir: String,
        include_hidden: bool,
    },
    RemoveDir {
        path: String,
        id: i32,
        recursive: bool,
    },
    RemoveFile {
        path: String,
        id: i32,
        file_num: i32,
    },
    CreateDir {
        path: String,
        id: i32,
    },
    NewWrite {
        path: String,
        id: i32,
        files: Vec<(String, u64)>,
    },
    CancelWrite {
        id: i32,
    },
    WriteBlock {
        id: i32,
        file_num: i32,
        data: Vec<u8>,
        compressed: bool,
    },
    WriteDone {
        id: i32,
        file_num: i32,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "t", content = "c")]
pub enum Data {
    Login {
        id: i32,
        is_file_transfer: bool,
        peer_id: String,
        name: String,
        authorized: bool,
        port_forward: String,
        keyboard: bool,
        clipboard: bool,
        audio: bool,
    },
    ChatMessage {
        text: String,
    },
    SwitchPermission {
        name: String,
        enabled: bool,
    },
    SystemInfo(Option<String>),
    Authorize,
    Close,
    SAS,
    OnlineStatus(Option<(i64, bool)>),
    Config((String, Option<String>)),
    Options(Option<HashMap<String, String>>),
    NatType(Option<i32>),
    ConfirmedKey(Option<(Vec<u8>, Vec<u8>)>),
    RawMessage(Vec<u8>),
    FS(FS),
    Test,
}

pub async fn start(postfix: &str) -> ResultType<()> {
    let mut incoming = new_listener(postfix).await?;
    loop {
        if let Some(result) = incoming.next().await {
            match result {
                Ok(stream) => {
                    let mut stream = Connection::new(stream);
                    tokio::spawn(async move {
                        loop {
                            match stream.next().await {
                                Err(_) => {
                                    break;
                                }
                                Ok(Some(data)) => {
                                    handle(data, &mut stream).await;
                                }
                                _ => {}
                            }
                        }
                    });
                }
                Err(_) => {
                    //log::error!("Couldn't get client: {:?}", err);
                }
            }
        }
    }
}

pub async fn new_listener(postfix: &str) -> ResultType<Incoming> {
    let path = Config::ipc_path(postfix);
    #[cfg(not(windows))]
    check_pid(postfix).await;
    let mut endpoint = Endpoint::new(path.clone());
    match SecurityAttributes::allow_everyone_create() {
        Ok(attr) => endpoint.set_security_attributes(attr),
        Err(_) => {
            //log::error!("Failed to set ipc{} security: {}", postfix, err),
        }
    };
    match endpoint.incoming() {
        Ok(incoming) => {
            //log::info!("Started ipc{} server at path: {}", postfix, &path);
            #[cfg(not(windows))]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o0777)).ok();
                write_pid(postfix);
            }
            Ok(incoming)
        }
        Err(err) => {
            //log::error!("Faild to start ipc{} server at path {}: {}",postfix,path,err);
            Err(err.into())
        }
    }
}

async fn handle(data: Data, stream: &mut Connection) {
    match data {
        Data::SystemInfo(_) => {
            //allow_err!(stream.send(&Data::SystemInfo(Some(info))).await);
        }
        Data::Close => {
            crate::server::input_service::fix_key_down_timeout_at_exit();
            std::process::exit(0);
        }
        Data::OnlineStatus(_) => {
            let x = config::ONLINE
                .lock()
                .unwrap()
                .values()
                .max()
                .unwrap_or(&0)
                .clone();
            let confirmed = Config::get_key_confirmed();
            allow_err!(stream.send(&Data::OnlineStatus(Some((x, confirmed)))).await);
        }
        Data::ConfirmedKey(None) => {
            let out = if Config::get_key_confirmed() {
                Some(Config::get_key_pair())
            } else {
                None
            };
            allow_err!(stream.send(&Data::ConfirmedKey(out)).await);
        }
        Data::Config((name, value)) => match value {
            None => {
                let value;
                if name == "id" {
                    value = Some(Config::get_id());
                } else if name == "password" {
                    value = Some(Config::get_password());
                } else if name == "salt" {
                    value = Some(Config::get_salt());
                } else if name == "rendezvous_server" {
                    value = Some(Config::get_rendezvous_server().to_string());
                } else {
                    value = None;
                }
                allow_err!(stream.send(&Data::Config((name, value))).await);
            }
            Some(value) => {
                if name == "id" {
                    Config::set_id(&value);
                } else if name == "password" {
                    Config::set_password(&value);
                } else if name == "salt" {
                    Config::set_salt(&value);
                } else {
                    return;
                }
            }
        },
        Data::Options(value) => match value {
            None => {
                let v = Config::get_options();
                allow_err!(stream.send(&Data::Options(Some(v))).await);
            }
            Some(value) => {
                Config::set_options(value);
            }
        },
        Data::NatType(_) => {
            let t = Config::get_nat_type();
            allow_err!(stream.send(&Data::NatType(Some(t))).await);
        }
        _ => {}
    }
}

pub async fn connect(ms_timeout: u64, postfix: &str) -> ResultType<ConnectionTmpl<ConnClient>> {
    let path = Config::ipc_path(postfix);
    let client = timeout(ms_timeout, Endpoint::connect(&path)).await??;
    Ok(ConnectionTmpl::new(client))
}

#[inline]
#[cfg(not(windows))]
fn get_pid_file(postfix: &str) -> String {
    let path = Config::ipc_path(postfix);
    format!("{}.pid", path)
}

#[cfg(not(windows))]
async fn check_pid(postfix: &str) {
    let pid_file = get_pid_file(postfix);
    if let Ok(mut file) = File::open(&pid_file) {
        let mut content = String::new();
        file.read_to_string(&mut content).ok();
        let pid = content.parse::<i32>().unwrap_or(0);
        if pid > 0 {
            if let Ok(p) = psutil::process::Process::new(pid as _) {
                if let Ok(current) = psutil::process::Process::current() {
                    if current.name().unwrap_or("".to_owned()) == p.name().unwrap_or("".to_owned())
                    {
                        if connect(1000, postfix).await.is_ok() {
                            return;
                        }
                    }
                }
            }
        }
    }
    hbb_common::allow_err!(std::fs::remove_file(&Config::ipc_path(postfix)));
}

#[inline]
#[cfg(not(windows))]
fn write_pid(postfix: &str) {
    let path = get_pid_file(postfix);
    if let Ok(mut file) = File::create(&path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o0777)).ok();
        file.write_all(&std::process::id().to_string().into_bytes())
            .ok();
    }
}

pub struct ConnectionTmpl<T> {
    inner: Framed<T, BytesCodec>,
}

pub type Connection = ConnectionTmpl<Conn>;

impl<T> ConnectionTmpl<T>
where
    T: AsyncRead + AsyncWrite + std::marker::Unpin,
{
    pub fn new(conn: T) -> Self {
        Self {
            inner: Framed::new(conn, BytesCodec::new()),
        }
    }

    pub async fn send(&mut self, data: &Data) -> ResultType<()> {
        let v = serde_json::to_vec(data)?;
        self.inner.send(bytes::Bytes::from(v)).await?;
        Ok(())
    }


    pub async fn next_timeout(&mut self, ms_timeout: u64) -> ResultType<Option<Data>> {
        Ok(timeout(ms_timeout, self.next()).await??)
    }


    pub async fn next(&mut self) -> ResultType<Option<Data>> {
        match self.inner.next().await {
            Some(res) => {
                let bytes = res?;
                if let Ok(s) = std::str::from_utf8(&bytes) {
                    if let Ok(data) = serde_json::from_str::<Data>(s) {
                        return Ok(Some(data));
                    }
                }
                return Ok(None);
            }
            _ => {
                bail!("reset by the peer");
            }
        }
    }
}

async fn get_config_async(name: &str, ms_timeout: u64) -> ResultType<Option<String>> {
    let mut c = connect(ms_timeout, "").await?;
    c.send(&Data::Config((name.to_owned(), None))).await?;
    if let Some(Data::Config((name2, value))) = c.next_timeout(ms_timeout).await? {
        if name == name2 {
            return Ok(value);
        }
    }
    return Ok(None);
}
pub async fn get_rendezvous_server(ms_timeout: u64) -> SocketAddr {
    if let Ok(Some(v)) = get_config_async("rendezvous_server", ms_timeout).await {
        if let Ok(v) = v.parse() {
            return v;
        }
    }
    return Config::get_rendezvous_server();
}