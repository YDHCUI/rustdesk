use hbb_common::{
    bytes::BytesMut,
    protobuf::Message as _,
    rendezvous_proto::*,
    tcp::{new_listener, FramedStream},
    tokio,
    udp::FramedSocket,
};

#[tokio::main(basic_scheduler)]
async fn main() {
    let argv: Vec<_> = std::env::args().collect();
    let mut opts = getopts::Options::new();
    opts.optopt("i", "ipaddr","", "");
    opts.optopt("t", "tcplisten","", "");
    opts.optopt("u", "udplisten","", "");

    let matches = match opts.parse(&argv[1..]) {
        Ok(m) => m,
        Err(_) => {
            return
        }
    };

    let mut relay_server;

    if let Some(ipaddr) = matches.opt_str("i"){
        relay_server = ipaddr;
    }else{
        println!("公网地址必须填写");
        return
    }
    let tcplisten = matches.opt_str("t").unwrap_or("0.0.0.0:21116".to_string());
    let udplisten = matches.opt_str("u").unwrap_or("0.0.0.0:21117".to_string());

    let mut socket = FramedSocket::new(tcplisten.clone()).await.unwrap();
    let mut listener = new_listener(tcplisten, false).await.unwrap();
    let mut rlistener = new_listener(udplisten, false).await.unwrap();
    let mut id_map = std::collections::HashMap::new();
    
    let mut saved_stream = None;
    loop {
        tokio::select! {
            Some(Ok((bytes, addr))) = socket.next() => {
                handle_udp(&mut socket, bytes, addr, &mut id_map).await;
            }
            Ok((stream, addr)) = listener.accept() => {
                let mut stream = FramedStream::from(stream);
                if let Some(Ok(bytes)) = stream.next_timeout(3000).await {
                    if let Ok(msg_in) = RendezvousMessage::parse_from_bytes(&bytes) {
                        println!("主动[{:?}]{:?}", addr, msg_in);
                        match msg_in.union {
                            Some(rendezvous_message::Union::punch_hole_request(ph)) => {
                                if let Some(addr) = id_map.get(&ph.id) {
                                    let mut msg_out = RendezvousMessage::new();
                                    msg_out.set_request_relay(RequestRelay {
                                        relay_server: relay_server.clone(),
                                        ..Default::default()
                                    });
                                    socket.send(&msg_out, addr.clone()).await.ok();
                                    saved_stream = Some(stream);
                                }
                            }
                            Some(rendezvous_message::Union::relay_response(_)) => {
                                let mut msg_out = RendezvousMessage::new();
                                msg_out.set_relay_response(RelayResponse {
                                    relay_server: relay_server.clone(),
                                    ..Default::default()
                                });
                                match saved_stream.take() {
                                    Some(mut stream) => {
                                        saved_stream = None;
                                        stream.send(&msg_out).await.ok();
                                        if let Ok((stream_a, a)) = rlistener.accept().await {
                                            let mut stream_a = FramedStream::from(stream_a);
                                            stream_a.next_timeout(3_000).await;
                                            if let Ok((stream_b, b)) = rlistener.accept().await {
                                                let mut stream_b = FramedStream::from(stream_b);
                                                stream_b.next_timeout(3_000).await;
                                                tokio::spawn(async move{
                                                    relay(stream_a, stream_b).await;
                                                });
                                            }
                                        } 
                                    }
                                    None => {  }
                                }
                            }
                            _ => {}
                        }
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


async fn handle_udp(
    socket: &mut FramedSocket,
    bytes: BytesMut,
    addr: std::net::SocketAddr,
    id_map: &mut std::collections::HashMap<String, std::net::SocketAddr>,
) {
    if let Ok(msg_in) = RendezvousMessage::parse_from_bytes(&bytes) {
        //println!("UDP {:?} {:?}", addr,msg_in);
        match msg_in.union {
            Some(rendezvous_message::Union::register_peer(rp)) => {
                id_map.insert(rp.id, addr);
                let mut msg_out = RendezvousMessage::new();
                msg_out.set_register_peer_response(RegisterPeerResponse::new());
                socket.send(&msg_out, addr).await.ok();
            }
            Some(rendezvous_message::Union::register_pk(_)) => {
                let mut msg_out = RendezvousMessage::new();
                msg_out.set_register_pk_response(RegisterPkResponse {
                    result: register_pk_response::Result::OK.into(),
                    ..Default::default()
                });
                socket.send(&msg_out, addr).await.ok();
            }
            _ => {}
        }
    }
}
