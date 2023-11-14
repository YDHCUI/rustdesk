pub use arboard::Clipboard as ClipboardContext;
use hbb_common::{
    allow_err,
    compress::{compress as compress_func, decompress},
    config::{Config, COMPRESS_LEVEL, RENDEZVOUS_TIMEOUT},
    message_proto::*,
    protobuf::Message as _,
    protobuf::ProtobufEnum,
    rendezvous_proto::*,
    tcp::FramedStream,
    tokio, ResultType,
};

use std::sync::{Arc, Mutex};

pub const CLIPBOARD_NAME: &'static str = "clipboard";
pub const CLIPBOARD_INTERVAL: u64 = 333;

lazy_static::lazy_static! {
    pub static ref CONTENT: Arc<Mutex<String>> = Default::default();
}

#[inline]
pub fn valid_for_numlock(evt: &KeyEvent) -> bool {
    if let Some(key_event::Union::control_key(ck)) = evt.union {
        let v = ck.value();
        (v >= ControlKey::Numpad0.value() && v <= ControlKey::Numpad9.value())
            || v == ControlKey::Decimal.value()
    } else {
        false
    }
}

#[inline]
pub fn valid_for_capslock(evt: &KeyEvent) -> bool {
    if let Some(key_event::Union::chr(ch)) = evt.union {
        ch >= 'a' as u32 && ch <= 'z' as u32
    } else {
        false
    }
}

pub fn create_clipboard_msg(content: String) -> Message {
    let bytes = content.into_bytes();
    let compressed = compress_func(&bytes, COMPRESS_LEVEL);
    let compress = compressed.len() < bytes.len();
    let content = if compress { compressed } else { bytes };
    let mut msg = Message::new();
    msg.set_clipboard(Clipboard {
        compress,
        content,
        ..Default::default()
    });
    msg
}

pub fn check_clipboard(
    ctx: &mut ClipboardContext,
    old: Option<&Arc<Mutex<String>>>,
) -> Option<Message> {
    let old = if let Some(old) = old { old } else { &CONTENT };
    if let Ok(content) = ctx.get_text() {
        if content.len() < 2_000_000 && !content.is_empty() {
            let changed = content != *old.lock().unwrap();
            if changed {
                *old.lock().unwrap() = content.clone();
                return Some(create_clipboard_msg(content));
            }
        }
    }
    None
}

pub fn update_clipboard(clipboard: Clipboard, old: Option<&Arc<Mutex<String>>>) {
    let content = if clipboard.compress {
        decompress(&clipboard.content)
    } else {
        clipboard.content
    };
    if let Ok(content) = String::from_utf8(content) {
        match ClipboardContext::new() {
            Ok(mut ctx) => {
                let _side = if old.is_none() { "host" } else { "client" };
                let old = if let Some(old) = old { old } else { &CONTENT };
                *old.lock().unwrap() = content.clone();
                allow_err!(ctx.set_text(content));
            }
            Err(_) => {}
        }
    }
}

#[cfg(feature = "use_rubato")]
pub fn resample_channels(
    data: &[f32],
    sample_rate0: u32,
    sample_rate: u32,
    channels: u16,
) -> Vec<f32> {
    use rubato::{
        InterpolationParameters, InterpolationType, Resampler, SincFixedIn, WindowFunction,
    };
    let params = InterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: InterpolationType::Nearest,
        oversampling_factor: 160,
        window: WindowFunction::BlackmanHarris2,
    };
    let mut resampler = SincFixedIn::<f64>::new(
        sample_rate as f64 / sample_rate0 as f64,
        params,
        data.len() / (channels as usize),
        channels as _,
    );
    let mut waves_in = Vec::new();
    if channels == 2 {
        waves_in.push(
            data.iter()
                .step_by(2)
                .map(|x| *x as f64)
                .collect::<Vec<_>>(),
        );
        waves_in.push(
            data.iter()
                .skip(1)
                .step_by(2)
                .map(|x| *x as f64)
                .collect::<Vec<_>>(),
        );
    } else {
        waves_in.push(data.iter().map(|x| *x as f64).collect::<Vec<_>>());
    }
    if let Ok(x) = resampler.process(&waves_in) {
        if x.is_empty() {
            Vec::new()
        } else if x.len() == 2 {
            x[0].chunks(1)
                .zip(x[1].chunks(1))
                .flat_map(|(a, b)| a.into_iter().chain(b))
                .map(|x| *x as f32)
                .collect()
        } else {
            x[0].iter().map(|x| *x as f32).collect()
        }
    } else {
        Vec::new()
    }
}

#[cfg(feature = "use_dasp")]
pub fn resample_channels(
    data: &[f32],
    sample_rate0: u32,
    sample_rate: u32,
    channels: u16,
) -> Vec<f32> {
    use dasp::{interpolate::linear::Linear, signal, Signal};
    let n = data.len() / (channels as usize);
    let n = n * sample_rate as usize / sample_rate0 as usize;
    if channels == 2 {
        let mut source = signal::from_interleaved_samples_iter::<_, [_; 2]>(data.iter().cloned());
        let a = source.next();
        let b = source.next();
        let interp = Linear::new(a, b);
        let mut data = Vec::with_capacity(n << 1);
        for x in source
            .from_hz_to_hz(interp, sample_rate0 as _, sample_rate as _)
            .take(n)
        {
            data.push(x[0]);
            data.push(x[1]);
        }
        data
    } else {
        let mut source = signal::from_iter(data.iter().cloned());
        let a = source.next();
        let b = source.next();
        let interp = Linear::new(a, b);
        source
            .from_hz_to_hz(interp, sample_rate0 as _, sample_rate as _)
            .take(n)
            .collect()
    }
}

#[cfg(feature = "use_samplerate")]
pub fn resample_channels(
    data: &[f32],
    sample_rate0: u32,
    sample_rate: u32,
    channels: u16,
) -> Vec<f32> {
    use samplerate::{convert, ConverterType};
    convert(
        sample_rate0 as _,
        sample_rate as _,
        channels as _,
        ConverterType::SincBestQuality,
        data,
    )
    .unwrap_or_default()
}


#[inline]
pub fn get_time() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0) as _
}

pub fn run_me<T: AsRef<std::ffi::OsStr>>(args: Vec<T>) -> std::io::Result<std::process::Child> {
    let cmd = std::env::current_exe()?;
    return std::process::Command::new(cmd).args(&args).spawn();
}

pub fn username() -> String {
    // fix bug of whoami
    return whoami::username().trim_end_matches('\0').to_owned();
}

#[inline]
pub fn check_port<T: std::string::ToString>(host: T, port: i32) -> String {
    let host = host.to_string();
    if !host.contains(":") {
        return format!("{}:{}", host, port);
    }
    return host;
}

pub const POSTFIX_SERVICE: &'static str = "_service";

#[inline]
pub fn is_control_key(evt: &KeyEvent, key: &ControlKey) -> bool {
    if let Some(key_event::Union::control_key(ck)) = evt.union {
        ck.value() == key.value()
    } else {
        false
    }
}

#[inline]
pub fn is_modifier(evt: &KeyEvent) -> bool {
    if let Some(key_event::Union::control_key(ck)) = evt.union {
        let v = ck.value();
        v == ControlKey::Alt.value()
            || v == ControlKey::Shift.value()
            || v == ControlKey::Control.value()
            || v == ControlKey::Meta.value()
            || v == ControlKey::RAlt.value()
            || v == ControlKey::RShift.value()
            || v == ControlKey::RControl.value()
            || v == ControlKey::RWin.value()
    } else {
        false
    }
}


use std::net::UdpSocket;
pub fn get_local_ipaddr() -> Option<String> {
    let socket = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(_) => return None,
    };

    match socket.connect("8.8.8.8:80") {
        Ok(()) => (),
        Err(_) => return None,
    };

    match socket.local_addr() {
        Ok(addr) => return Some(addr.ip().to_string()),
        Err(_) => return None,
    };
}

pub async fn get_rendezvous_server(ms_timeout: u64) -> std::net::SocketAddr {
    crate::ipc::get_rendezvous_server(ms_timeout).await
}

pub fn test_nat_type() {
    std::thread::spawn(move || loop {
        match test_nat_type_() {
            Ok(true) => break,
            Err(_) => {
                //log::error!("test nat: {}", err);
            }
            _ => {}
        }
        if Config::get_nat_type() != 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(12));
    });
}

#[tokio::main(flavor = "current_thread")]
async fn test_nat_type_() -> ResultType<bool> {
    let rendezvous_server = get_rendezvous_server(100).await;
    let server1 = rendezvous_server;
    let mut server2 = server1;
    server2.set_port(server1.port() - 1);
    let mut msg_out = RendezvousMessage::new();
    let serial = Config::get_serial();
    msg_out.set_test_nat_request(TestNatRequest {
        serial,
        ..Default::default()
    });
    let mut port1 = 0;
    let mut port2 = 0;
    let mut addr = Config::get_any_listen_addr();
    for i in 0..2 {
        let mut socket = FramedStream::new(
            if i == 0 { &server1 } else { &server2 },
            addr,
            RENDEZVOUS_TIMEOUT,
        )
        .await?;
        addr = socket.get_ref().local_addr()?;
        socket.send(&msg_out).await?;
        if let Some(Ok(bytes)) = socket.next_timeout(3000).await {
            if let Ok(msg_in) = RendezvousMessage::parse_from_bytes(&bytes) {
                if let Some(rendezvous_message::Union::test_nat_response(tnr)) = msg_in.union {
                    if i == 0 {
                        port1 = tnr.port;
                    } else {
                        port2 = tnr.port;
                    }
                    if let Some(cu) = tnr.cu.as_ref() {
                        Config::set_option(
                            "rendezvous-servers".to_owned(),
                            cu.rendezvous_servers.join(","),
                        );
                        Config::set_serial(cu.serial);
                    }
                }
            }
        } else {
            break;
        }
    }
    let ok = port1 > 0 && port2 > 0;
    if ok {
        let t = if port1 == port2 {
            NatType::ASYMMETRIC
        } else {
            NatType::SYMMETRIC
        };
        Config::set_nat_type(t as _);
        //log::info!("Tested nat type: {:?} in {:?}", t, start.elapsed());
    }
    Ok(ok)
}