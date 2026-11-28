//! SPA 互通真机回归入口（examples/spa_send.rs）。
//!
//! 用 libfko（与 easyrdp-client SPA 生产路径同一实现）构造原生 SPA 包并发送：
//!
//! ```text
//! cargo run -p easyrdp-core --example spa_send -- <host> <port> <key> <hmac_key> [outfile]
//! ```
//!
//! * host/port：fwknopd 地址与端口（WSL2 上 `fwknopd --udp-server -p 62201`）。
//! * key/hmac_key：fwknop 明文裸密钥（对应服务器 access.conf `KEY:`/`HMAC_KEY:`）。
//! * outfile：可选——只把包内容写到文件（供 WSL 侧 python 代发，绕过
//!   Windows→WSL2 UDP localhost 限制），不发网络包。
//!
//! 与生产代码共享 `knock::KnockConfig` 路径（build_spa_packet），保证回归
//! 覆盖的就是上线实现。

use easyrdp_core::knock::KnockConfig;
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("用法: spa_send <host> <port> <key> <hmac_key> [outfile]");
        std::process::exit(2);
    }
    let host = &args[1];
    let port: u16 = args[2].parse().expect("端口非法");
    let key = &args[3];
    let hmac_key = &args[4];
    let outfile = args.get(5);

    let cfg = KnockConfig {
        enabled: true,
        spa_port: port,
        key: key.clone(),
        hmac_key: hmac_key.clone(),
        open_ports: "tcp/3389".to_string(),
        ..KnockConfig::default()
    };
    let packet = easyrdp_core::knock::example_build_packet(host, &cfg)
        .expect("SPA 包构造失败");
    eprintln!("[spa_send] 包长度 {} 字节", packet.len());

    match outfile {
        Some(path) => {
            let mut f = std::fs::File::create(path).expect("写包文件失败");
            f.write_all(&packet).expect("写包失败");
            eprintln!("[spa_send] 包已写入 {path}");
        }
        None => {
            use std::net::UdpSocket;
            let sock = UdpSocket::bind("0.0.0.0:0").expect("socket 失败");
            let n = sock
                .send_to(&packet, format!("{host}:{port}"))
                .expect("发送失败");
            eprintln!("[spa_send] 已发送 {n} 字节 → {host}:{port}");
        }
    }
}
