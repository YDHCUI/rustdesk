
use rand::Rng;
use serde_derive::{Deserialize, Serialize};
use sodiumoxide::crypto::sign;
use std::{
    fs,
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

pub const APP_NAME: &str = "agent";
pub const BIND_INTERFACE: &str = "0.0.0.0";
pub const RENDEZVOUS_TIMEOUT: u64 = 12_000;
pub const CONNECT_TIMEOUT: u64 = 18_000;
pub const COMPRESS_LEVEL: i32 = 3;
const SERIAL: i32 = 1;

#[cfg(target_os = "macos")]
pub const ORG: &str = "com.rsc2.agent";
pub const RENDEZVOUS_SERVERS: &'static [&'static str] = &[
    //"rs-ny.rustdesk.com",
    //"rs-sg.rustdesk.com",
    "192.168.93.182",
    //"119.29.137.232",
    //"rs-cn.rustdesk.com"
];
pub const RENDEZVOUS_PORT: i32 = 21116;
pub const RELAY_PORT: i32 = 21117;

type Size = (i32, i32, i32, i32);

const CHARS: &'static [char] = &[
    '2', '3', '4', '5', '6', '7', '8', '9', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k',
    'm', 'n', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z',
];

lazy_static::lazy_static! {
    static ref CONFIG: Arc<RwLock<Config>> = Arc::new(RwLock::new(Config::new::<Config>()));
    pub static ref ONLINE: Arc<Mutex<HashMap<String, i64>>> = Default::default();
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Config {
    #[serde(default)]
    id: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    salt: String,
    #[serde(default)]
    key_pair: (Vec<u8>, Vec<u8>), // sk, pk
    #[serde(default)]
    key_confirmed: bool,
    #[serde(default)]
    keys_confirmed: HashMap<String, bool>,

    #[serde(default)]
    remote_id: String, // latest used one
    #[serde(default)]
    size: Size,
    #[serde(default)]
    rendezvous_server: String,
    #[serde(default)]
    nat_type: i32,
    #[serde(default)]
    serial: i32,

    // the other scalar value must before this
    #[serde(default)]
    pub options: HashMap<String, String>,
}

fn patch(path: PathBuf) -> PathBuf {
    if let Some(_tmp) = path.to_str() {
        #[cfg(windows)]
        return _tmp
            .replace(
                "system32\\config\\systemprofile",
                "ServiceProfiles\\LocalService",
            )
            .into();
        #[cfg(target_os = "macos")]
        return _tmp.replace("Application Support", "Preferences").into();
        #[cfg(target_os = "linux")]
        {
            if _tmp == "/root" {
                if let Ok(output) = std::process::Command::new("whoami").output() {
                    let user = String::from_utf8_lossy(&output.stdout).to_string().trim().to_owned();
                    if user != "root" {
                        return format!("/home/{}", user).into();
                    }
                }
            }
        }
    }
    path
}


impl Config {
    fn new<T: serde::Serialize + serde::de::DeserializeOwned + Default + std::fmt::Debug>() -> T {
        T::default()
    }

    pub fn get_home() -> PathBuf {
        if let Some(path) = dirs_next::home_dir() {
            patch(path)
        } else if let Ok(path) = std::env::current_dir() {
            path
        } else {
            std::env::temp_dir()
        }
    }


    pub fn ipc_path(postfix: &str) -> String {
        #[cfg(windows)]{
            format!("\\\\.\\pipe\\{}\\query{}", APP_NAME, postfix)
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut path: PathBuf = format!("/tmp/{}", APP_NAME).into();
            fs::create_dir(&path).ok();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o0777)).ok();
            path.push(format!("ipc{}", postfix));
            path.to_str().unwrap_or("").to_owned()
        }
    }

    #[inline]
    pub fn get_any_listen_addr() -> SocketAddr {
        format!("{}:0", BIND_INTERFACE).parse().unwrap()
    }

    pub fn get_rendezvous_server() -> SocketAddr {
        let mut rendezvous_server = Self::get_option("custom-rendezvous-server");
        if rendezvous_server.is_empty() {
            rendezvous_server = CONFIG.read().unwrap().rendezvous_server.clone();
        }
        if rendezvous_server.is_empty() {
            rendezvous_server = Self::get_rendezvous_servers()
                .drain(..)
                .next()
                .unwrap_or("".to_owned());
        }
        if !rendezvous_server.contains(":") {
            rendezvous_server = format!("{}:{}", rendezvous_server, RENDEZVOUS_PORT);
        }
        if let Ok(addr) = crate::to_socket_addr(&rendezvous_server) {
            addr
        } else {
            Self::get_any_listen_addr()
        }
    }

    pub fn get_rendezvous_servers() -> Vec<String> {
        let s = Self::get_option("custom-rendezvous-server");
        if !s.is_empty() {
            return vec![s];
        }
        let serial_obsolute = CONFIG.read().unwrap().serial > SERIAL;
        if serial_obsolute {
            let ss: Vec<String> = Self::get_option("rendezvous-servers")
                .split(",")
                .filter(|x| x.contains("."))
                .map(|x| x.to_owned())
                .collect();
            if !ss.is_empty() {
                return ss;
            }
        }
        return RENDEZVOUS_SERVERS.iter().map(|x| x.to_string()).collect();
    }

    pub fn reset_online() {
        *ONLINE.lock().unwrap() = Default::default();
    }

    pub fn update_latency(host: &str, latency: i64) {
        ONLINE.lock().unwrap().insert(host.to_owned(), latency);
        let mut host = "".to_owned();
        let mut delay = i64::MAX;
        for (tmp_host, tmp_delay) in ONLINE.lock().unwrap().iter() {
            if tmp_delay > &0 && tmp_delay < &delay {
                delay = tmp_delay.clone();
                host = tmp_host.to_string();
            }
        }
        if !host.is_empty() {
            let mut config = CONFIG.write().unwrap(); 
            if host != config.rendezvous_server {
                config.rendezvous_server = host;
            }
        }
    }

    pub fn set_nat_type(nat_type: i32) {
        let mut config = CONFIG.write().unwrap();
        if nat_type == config.nat_type {
            return;
        }
        config.nat_type = nat_type;
    }

    pub fn get_nat_type() -> i32 {
        CONFIG.read().unwrap().nat_type
    }

    pub fn set_serial(serial: i32) {
        let mut config = CONFIG.write().unwrap();
        if serial == config.serial {
            return;
        }
        config.serial = serial;
    }

    pub fn get_serial() -> i32 {
        std::cmp::max(CONFIG.read().unwrap().serial, SERIAL)
    }

    pub fn get_key_confirmed() -> bool {
        CONFIG.read().unwrap().key_confirmed
    }

    pub fn set_key_confirmed(v: bool) {
        let mut config = CONFIG.write().unwrap();
        if config.key_confirmed == v {
            return;
        }
        config.key_confirmed = v;
        if !v {
            config.keys_confirmed = Default::default();
        }
    }

    pub fn get_host_key_confirmed(host: &str) -> bool {
        if let Some(true) = CONFIG.read().unwrap().keys_confirmed.get(host) {
            true
        } else {
            false
        }
    }

    pub fn set_host_key_confirmed(host: &str, v: bool) {
        if Self::get_host_key_confirmed(host) == v {
            return;
        }
        let mut config = CONFIG.write().unwrap();
        config.keys_confirmed.insert(host.to_owned(), v);
    }

    pub fn set_key_pair(pair: (Vec<u8>, Vec<u8>)) {
        let mut config = CONFIG.write().unwrap();
        config.key_pair = pair;
    }

    pub fn get_key_pair() -> (Vec<u8>, Vec<u8>) {
        // lock here to make sure no gen_keypair more than once
        let mut config = CONFIG.write().unwrap();
        if config.key_pair.0.is_empty() {
            let (pk, sk) = sign::gen_keypair();
            config.key_pair = (sk.0.to_vec(), pk.0.into());
        }
        config.key_pair.clone()
    }

    pub fn get_options() -> HashMap<String, String> {
        CONFIG.read().unwrap().options.clone()
    }

    pub fn set_options(v: HashMap<String, String>) {
        let mut config = CONFIG.write().unwrap();
        config.options = v;
    }

    pub fn get_option(k: &str) -> String {
        if let Some(v) = CONFIG.read().unwrap().options.get(k) {
            v.clone()
        } else {
            "".to_owned()
        }
    }

    pub fn set_option(k: String, v: String) {
        let mut config = CONFIG.write().unwrap();
        if k == "custom-rendezvous-server" {
            config.rendezvous_server = "".to_owned();
        }
        let v2 = if v.is_empty() { None } else { Some(&v) };
        if v2 != config.options.get(&k) {
            if v2.is_none() {
                config.options.remove(&k);
            } else {
                config.options.insert(k, v);
            }
        }
    }

    pub fn set_salt(salt: &str) {
        let mut config = CONFIG.write().unwrap();
        if salt == config.salt {
            return;
        }
        config.salt = salt.into();
    }

    pub fn get_salt() -> String {
        let mut salt = CONFIG.read().unwrap().salt.clone();
        if salt.is_empty() {
            let mut rng = rand::thread_rng();
            let new_salt = (0..8).map(|_| CHARS[rng.gen::<usize>() % CHARS.len()]).collect();
            salt = new_salt;
            Config::set_salt(&salt);
        }
        salt
    }
    
    pub fn set_id(id: &str) {
        let mut config = CONFIG.write().unwrap();
        if id == config.id {
            return;
        }
        config.id = id.into();
    }

    pub fn get_id() -> String {
        let mut id = "agentuseridtest".to_string(); //CONFIG.read().unwrap().id.clone();
        if id.is_empty() {
            let mut rng = rand::thread_rng();
            let new_id = (0..16).map(|_| CHARS[rng.gen::<usize>() % CHARS.len()]).collect();
            id = new_id;
            Config::set_id(&id);
            println!("id:{:?}", id);
        }
        id
    }

    pub fn set_password(password: &str) {
        let mut config = CONFIG.write().unwrap();
        if password == config.password {
            return;
        }
        config.password = password.into();
    }

    pub fn get_password() -> String {
        let mut password = "666666".to_string();//CONFIG.read().unwrap().password.clone();
        if password.is_empty() {
            let mut rng = rand::thread_rng();
            let new_pwd = (0..8).map(|_| CHARS[rng.gen::<usize>() % CHARS.len()]).collect();
            password = new_pwd;
            Config::set_password(&password);
            println!("pw:{:?}", password);
        }
        password
    }

}
