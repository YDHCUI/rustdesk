// Specify the Windows subsystem to eliminate console window.
// Requires Rust 1.18.
//#![windows_subsystem = "windows"]

const VERSION: &str = "1.1.8";

mod platform;
mod server;
mod rendezvous;
mod common;
mod cm;
mod ipc;


use hbb_common::config::Config;
use hbb_common::tokio;
use hbb_common::tokio::runtime::Runtime;

/*
#[no_mangle]
pub unsafe extern "C" fn plugmain(){ 
    println!("{:#?}",std::env::args());
    Config::set_option("custom-rendezvous-server".to_string(),"192.168.93.217".to_string());
    Config::set_id("asdfghjkl");
    Config::set_password("666666");
    let mut rt = Runtime::new().expect("runtime err");
    rt.block_on(async {
        tokio::spawn(async move {
            crate::cm::start_ipc(cm::ConnectionManager::new()).await;
        });
        crate::rendezvous::RendezvousMediator::start_all().await;
    });
}
*/

#[tokio::main]
async fn main() {
    let argv: Vec<_> = std::env::args().collect();
    let mut opts = getopts::Options::new();
    opts.optopt("s", "server","", "");
    opts.optopt("u", "username","", "");
    opts.optopt("p", "password","", "");
    //opts.optopt("sp", "RENDEZVOUS_PORT","", "");
    //opts.optopt("rp", "RELAY_PORT","", "");
    //
    let matches = match opts.parse(&argv[1..]) {
        Ok(m) => m,
        Err(_) => {
            return
        }
    };

    if let Some(s) = matches.opt_str("s"){
        Config::set_option("custom-rendezvous-server".to_string(),s);
    }
    if let Some(u) = matches.opt_str("u"){
        Config::set_id(&u);
    }
    if let Some(p) = matches.opt_str("p"){
        Config::set_password(&p);
    }
    tokio::spawn(async move {
        if let Err(err) = crate::ipc::start("").await {
            println!("{:?}",err);
            std::process::exit(-1);
        }
    });
    tokio::spawn(async move {
        crate::cm::start_ipc(cm::ConnectionManager::new()).await;
    }); 
    crate::rendezvous::RendezvousMediator::start_all().await; 
}
