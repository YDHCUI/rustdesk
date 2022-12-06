use hbb_common::{
    bytes::BytesMut,
    protobuf::Message as _,
    rendezvous_proto::*,
    tcp::{new_listener, FramedStream},
    tokio,
    tokio::time::interval,
    tokio::time::Duration,
    udp::FramedSocket,
    log,
    env_logger::*,
    protobuf,
    AddrMangle,
};

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{Arc, Mutex, RwLock},
    time::{SystemTime,UNIX_EPOCH},
};
use serde_derive::{Deserialize, Serialize};
use uuid::Uuid;
use async_channel::{bounded, unbounded, Receiver, Sender};


#[derive(Debug, Clone)]
struct Agent {
    timestamp: u64,
    addr: SocketAddr,
    serial: i32,
    uuid: Vec<u8>,
    pk: Vec<u8>,
    hostname: String,
    username: String,
    platform: String,
    localaddr: String,
    version: String,
}
impl Agent {
    fn update(&mut self, addr: SocketAddr, serial: i32){
        self.addr = addr;
        self.serial = serial;
        self.timestamp = get_time() as u64;
    }
}

#[derive(Debug, Clone)]
struct Client {
    timestamp: u64,
    addr: SocketAddr,
    apikey: String,
}
impl Client {
    fn update(&mut self, addr: SocketAddr){
        self.addr = addr;
        self.timestamp = get_time() as u64;
    }
}

type Agents = HashMap<String, Agent>;
type Clients = HashMap<String, Client>;

pub fn get_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0) as u64
}


#[tokio::main]
async fn main() {
    init_from_env(Env::default().filter_or(DEFAULT_FILTER_ENV, "info"));
    let relay_server = std::env::var("IP").unwrap();
    let mut socket = FramedSocket::new("0.0.0.0:21116").await.unwrap();
    let mut active_listener = new_listener("0.0.0.0:21116", false).await.unwrap();
    let mut passive_listener = new_listener("0.0.0.0:21117", false).await.unwrap();

    let mut agents: Agents = HashMap::<String,Agent>::new();
    let mut clients: Clients = HashMap::<String,Client>::new();

    let mut active_stream: Option<FramedStream> = None;
    let mut passive_stream: Option<FramedStream> = None;
    
    loop {
        tokio::select! { 
            //UDP 21116
            Some(Ok((bytes, addr))) = socket.next() => {
                if let Ok(msg_in) = RendezvousMessage::parse_from_bytes(&bytes) {
                    log::info!("UDP [{:?}]{:?}", addr, msg_in);
                    match msg_in.union { 
                        Some(rendezvous_message::Union::client_login(cl)) => {
                            if cl.password == "PASSWORD" {
                                let apikey = format!("apikey-{}",&cl.id);
                                clients.insert(cl.id, Client {
                                    timestamp: get_time() as u64,
                                    addr: addr,
                                    apikey: apikey.clone()
                                });
                                let mut msg_out = RendezvousMessage::new();
                                msg_out.set_client_login_response(ClientLoginResponse{
                                    apikey: apikey,
                                    ..Default::default()
                                });
                                socket.send(&msg_out, addr).await.ok();
                            }
                        }

                        Some(rendezvous_message::Union::client_get_agent(ga)) => {
                            if let Some(client) = clients.get_mut(&ga.id) {
                                client.update(addr);
                                if ga.apikey == client.apikey {
                                    let mut msg_out = RendezvousMessage::new();
                                    let mut ret = Vec::new();
                                    for (id,ag) in &agents {
                                        ret.push(RegisterPk{
                                            id: id.clone(), 
                                            uuid: ag.uuid.clone(),
                                            pk: ag.pk.clone(),
                                            username: ag.username.clone(),
                                            hostname: ag.hostname.clone(), 
                                            platform: ag.platform.clone(),
                                            localaddr: ag.localaddr.clone(),
                                            version: ag.version.clone(),
                                            ..Default::default()
                                        });
                                    };
                                    msg_out.set_client_get_agent_response(ClientGetAgentResponse{
                                        agents: ret,
                                        ..Default::default()
                                    });
                                    socket.send(&msg_out, addr).await.ok();
                                }
                            };
                         } 

                        //agent心跳包，
                        Some(rendezvous_message::Union::register_peer(rp)) => {
                            let mut request_pk : bool = true; 
                            if let Some(agent) = agents.get_mut(&rp.id) {
                                request_pk = false;
                                agent.update(addr,rp.serial);
                            };
                            let mut msg_out = RendezvousMessage::new();
                            msg_out.set_register_peer_response(RegisterPeerResponse{
                                request_pk: request_pk, //是否回复初始化包
                                ..Default::default()
                            });
                            socket.send(&msg_out, addr).await.ok();
                        }
                        //agent初始化
                        Some(rendezvous_message::Union::register_pk(rp)) => {
                            agents.insert(rp.id, Agent {
                                timestamp: get_time() as u64,
                                addr: addr,
                                serial: 1,
                                uuid: rp.uuid,
                                pk: rp.pk,
                                hostname: rp.hostname,
                                username: rp.username,
                                platform: rp.platform,
                                localaddr: rp.localaddr,
                                version: rp.version,
                            });
                            
                            let mut msg_out = RendezvousMessage::new();
                            msg_out.set_register_pk_response(RegisterPkResponse {
                                result: register_pk_response::Result::OK.into(),
                                ..Default::default()
                            });
                            socket.send(&msg_out, addr).await.ok();
                        }
                        _ => { }
                    }
                }
            }
            //主动连接21116
            Ok((stream, addr)) = active_listener.accept() => {
                let mut stream = FramedStream::from(stream);
                if let Some(Ok(bytes)) = stream.next_timeout(3000).await {
                    if let Ok(msg_in) = RendezvousMessage::parse_from_bytes(&bytes) {
                        log::info!("主动[{:?}]{:?}", addr, msg_in);
                        match msg_in.union {
                            //网络类型测试
                            Some(rendezvous_message::Union::test_nat_request(ph)) => {
                                let mut msg_out = RendezvousMessage::new();
                                msg_out.set_test_nat_response(TestNatResponse {
                                    port: 0,
                                    cu: protobuf::MessageField::some(ConfigUpdate {
                                        ..Default::default()
                                    }),
                                    ..Default::default()
                                });
                                stream.send(&msg_out).await.ok();
                            }
                            //client 初次请求 
                            Some(rendezvous_message::Union::punch_hole_request(phr)) => {
                                if let Some(agent) = agents.get(&phr.id){
                                    let mut msg_out = RendezvousMessage::new();
                                    msg_out.set_request_relay(RequestRelay {
                                        socket_addr: AddrMangle::encode(addr),
                                        relay_server: relay_server.clone(),
                                        ..Default::default()
                                    });
                                    socket.send(&msg_out, agent.addr.clone()).await.ok();
                                    active_stream = Some(stream);
                                }else{
                                    let mut msg_out = RendezvousMessage::new();
                                    msg_out.set_punch_hole_response(PunchHoleResponse {
                                        failure: protobuf::ProtobufEnumOrUnknown::from(
                                            punch_hole_response::Failure::OFFLINE,
                                        ),
                                        ..Default::default()
                                    });
                                    stream.send(&msg_out).await.ok();
                                }
                            }
                            Some(rendezvous_message::Union::relay_response(rr)) => {
                                match active_stream.take() {
                                    Some(mut stream) => {
                                        active_stream = None;
                                        let mut msg_out = RendezvousMessage::new();
                                        msg_out.set_relay_response(RelayResponse {
                                            relay_server: relay_server.clone(),
                                            ..Default::default()
                                        });
                                        stream.send(&msg_out).await.ok();
                                    }
                                    None => { }
                                }
                            }
                            _ => { }
                        }
                    }
                }
            }
            
            Ok((stream, addr)) = passive_listener.accept() => {
                log::info!("被动[{:?}]", addr);
                let mut stream_agent = FramedStream::from(stream);
                match passive_stream.take() {
                    Some(mut stream_client) => {
                        passive_stream = None;
                        stream_agent.next_timeout(3_000).await;
                        stream_client.next_timeout(3_000).await;
                        tokio::spawn(async move{
                            relay(stream_agent, stream_client).await;
                        });
                    } 
                    None => {
                        passive_stream = Some(stream_agent);
                    }
                }
            }
        }
    }
}


async fn relay(
    stream: FramedStream,
    peer: FramedStream,
) {
    let mut peer = peer;
    let mut stream = stream;
    loop {
        tokio::select! {
            res = peer.next_timeout(3_000) => {
                if let Some(Ok(bytes)) = res {
                    stream.send_bytes(bytes.into()).await.ok();
                } else {
                    break;
                }
            },
            res = stream.next_timeout(3_000) => {
                if let Some(Ok(bytes)) = res {
                    peer.send_bytes(bytes.into()).await.ok();
                } else {
                    break;
                }
            },
        }
    }
}

